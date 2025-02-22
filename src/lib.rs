// Based on: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::sync::Mutex;

type ArcMutex<T> = Arc<Mutex<T>>;
const LOCK_ERROR: &str = "Failed to lock mutex";
const DOUBLE_RESOLVE_ERROR: &str = "Promise can only be resolved once";
const INNER_DOUBLE_RESOLVE_ERROR: &str = "Internal Error: Promise seems to be resolved twice";

#[inline(always)]
fn get_inner<T>(data: ArcMutex<T>) -> T {
    Arc::try_unwrap(data)
        .unwrap_or_else(|el| {
            panic!(
                "Failed to unwrap Arc to get inner (reference count: strong={}, weak={})",
                Arc::strong_count(&el),
                Arc::weak_count(&el)
            )
        })
        .into_inner()
        .expect(LOCK_ERROR)
}

#[inline(always)]
fn get_inner_multi<T>(data: ArcMutex<Option<T>>) -> T {
    data.lock()
        .expect(LOCK_ERROR)
        .take()
        .expect(DOUBLE_RESOLVE_ERROR)
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum State<T, E = ()> {
    Pending,
    Fulfilled(T),
    Rejected(E),
}

impl<T, E> State<T, E> {
    pub fn unwrap(self) -> T {
        match self {
            State::Fulfilled(value) => value,
            _ => panic!("Invalid unwrap, state is not Fulfilled"),
        }
    }

    pub fn unwrap_err(self) -> E {
        match self {
            State::Rejected(error) => error,
            _ => panic!("Invalid unwrap_err, state is not Rejected"),
        }
    }
}

struct PromiseInner<T, E> {
    state: State<T, E>,
    callback_fullfilled: Option<Box<dyn FnOnce(T) + Send>>,
    callback_rejected: Option<Box<dyn FnOnce(E) + Send>>,
}

pub struct Promise<T, E> {
    promise: ArcMutex<PromiseInner<T, E>>,
}

impl<T: Send, E: Send> Promise<T, E> {
    pub fn new<'a>(
        executor: impl FnOnce(Box<dyn FnOnce(T) + Send + 'a>, Box<dyn FnOnce(E) + Send + 'a>) + 'a,
    ) -> Self
    where
        T: 'a,
        E: 'a,
    {
        let promise = Arc::new(Mutex::new(PromiseInner {
            state: State::Pending,
            callback_fullfilled: None,
            callback_rejected: None,
        }));
        let promise_completion = promise.clone();
        let promise_reject = promise.clone();

        let single_call_ensure_completion = Arc::new(AtomicBool::new(false));
        let single_call_ensure_reject = single_call_ensure_completion.clone();

        executor(
            Box::new(move |value| {
                if single_call_ensure_completion.swap(true, Relaxed) {
                    panic!("{}", DOUBLE_RESOLVE_ERROR);
                }

                let PromiseInner {
                    state,
                    callback_fullfilled,
                    ..
                } = &mut *promise_completion.lock().expect(LOCK_ERROR);
                match callback_fullfilled.take() {
                    Some(callback) => callback(value),
                    None => *state = State::Fulfilled(value),
                }
            }),
            Box::new(move |error| {
                if single_call_ensure_reject.swap(true, Relaxed) {
                    panic!("{}", DOUBLE_RESOLVE_ERROR);
                }

                let PromiseInner {
                    state,
                    callback_rejected,
                    ..
                } = &mut *promise_reject.lock().expect(LOCK_ERROR);
                match callback_rejected.take() {
                    Some(callback) => callback(error),
                    None => *state = State::Rejected(error),
                }
            }),
        );
        Self { promise }
    }

    pub fn resolve(value: T) -> Self {
        Self::new(|resolve, _reject| {
            resolve(value);
        })
    }

    pub fn reject(error: E) -> Self {
        Self::new(|_resolve, reject| {
            reject(error);
        })
    }

    pub fn state(&self) -> State<T, E>
    where
        T: Clone,
        E: Clone,
    {
        self.promise.lock().expect(LOCK_ERROR).state.clone()
    }

    pub fn then<U: Send>(self, on_fulfilled: impl FnOnce(T) -> U + Send + 'static) -> Promise<U, E>
    where
        T: 'static,
        U: 'static,
        E: 'static,
    {
        Promise::new(move |resolve: Box<dyn FnOnce(U) + Send>, reject| {
            let on_fulfilled = move |value| {
                let new_value = on_fulfilled(value);
                resolve(new_value);
            };

            {
                let guard = self.promise.clone();
                let mut inner = guard.lock().expect(LOCK_ERROR);
                if let State::Pending = inner.state {
                    inner.callback_fullfilled.replace(Box::new(on_fulfilled));
                    inner.callback_rejected.replace(Box::new(reject));
                    return;
                }
            }
            match get_inner(self.promise).state {
                State::Pending => unreachable!("Pending state is handled before"),
                State::Fulfilled(value) => on_fulfilled(value),
                State::Rejected(error) => reject(error),
            }
        })
    }

    pub fn catch<F: Send>(self, on_rejected: impl FnOnce(E) -> F + Send + 'static) -> Promise<T, F>
    where
        T: 'static,
        F: 'static,
        E: 'static,
    {
        Promise::new(move |resolve, reject| {
            let on_rejected = move |error| {
                let new_error = on_rejected(error);
                reject(new_error);
            };

            {
                let guard = self.promise.clone();
                let mut inner = guard.lock().expect(LOCK_ERROR);
                if let State::Pending = inner.state {
                    inner.callback_fullfilled.replace(Box::new(resolve));
                    inner.callback_rejected.replace(Box::new(on_rejected));
                    return;
                }
            }

            match get_inner(self.promise).state {
                State::Pending => unreachable!("Pending state is handled before"),
                State::Fulfilled(value) => resolve(value),
                State::Rejected(error) => on_rejected(error),
            }
        })
    }

    pub fn finally(self, on_finally: impl FnOnce() + Send + 'static) -> Promise<T, E>
    where
        T: 'static,
        E: 'static,
    {
        Promise::new(move |resolve, reject| {
            {
                let guard = self.promise.clone();
                let mut inner = guard.lock().expect(LOCK_ERROR);
                if let State::Pending = inner.state {
                    let on_finally_fullfilled = Arc::new(Mutex::new(Some(on_finally)));
                    let on_finally_rejected = on_finally_fullfilled.clone();

                    inner.callback_fullfilled.replace(Box::new(move |value| {
                        resolve(value);

                        let on_finally = get_inner_multi(on_finally_fullfilled);
                        on_finally();
                    }));
                    inner.callback_rejected.replace(Box::new(move |error| {
                        reject(error);

                        let on_finally = get_inner_multi(on_finally_rejected);
                        on_finally();
                    }));
                    return;
                }
            }

            match get_inner(self.promise).state {
                State::Pending => unreachable!("Pending state is handled before"),
                State::Fulfilled(value) => resolve(value),
                State::Rejected(error) => reject(error),
            }
            on_finally();
        })
    }

    pub fn all(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<Vec<T>, E>
    where
        T: 'static,
        E: 'static,
    {
        // Last promise to resolve is responsible for resolving the all promise
        let fullfilled = Arc::new(Mutex::new(Vec::new()));
        Promise::new(move |resolve, reject| {
            let resolve = Arc::new(Mutex::new(Some(resolve)));
            let reject = Arc::new(Mutex::new(Some(reject)));
            let is_resolved = Arc::new(AtomicBool::new(false));

            let promises: Vec<_> = promises.into_iter().collect();
            let n_promises = promises.len();

            for promise in promises {
                let fullfilled = fullfilled.clone();

                let resolve = resolve.clone();
                let reject = reject.clone();

                let is_resolved_fullfilled = is_resolved.clone();
                let is_resolved_reject = is_resolved.clone();

                {
                    let guard = promise.promise.clone();
                    let mut inner = guard.lock().expect(LOCK_ERROR);
                    if let State::Pending = inner.state {
                        inner.callback_fullfilled.replace(Box::new(move |value| {
                            if is_resolved_fullfilled.load(Relaxed) {
                                return;
                            }

                            let mut fullfilled_ref = fullfilled.lock().expect(LOCK_ERROR);
                            fullfilled_ref.push(value);

                            if fullfilled_ref.len() == n_promises {
                                drop(fullfilled_ref);
                                let resolve = get_inner_multi(resolve);
                                resolve(get_inner(fullfilled));
                            }
                        }));
                        inner.callback_rejected.replace(Box::new(move |error| {
                            if is_resolved_reject.swap(true, Relaxed) {
                                return;
                            }

                            let reject = get_inner_multi(reject);
                            reject(error);
                        }));
                        continue;
                    }
                }

                match get_inner(promise.promise).state {
                    State::Pending => unreachable!("Pending state is handled before"),
                    State::Fulfilled(value) => {
                        if is_resolved.load(Relaxed) {
                            return;
                        }
                        fullfilled.lock().expect(LOCK_ERROR).push(value);
                    }
                    State::Rejected(error) => {
                        if is_resolved_reject.swap(true, Relaxed) {
                            return;
                        }
                        let reject = get_inner_multi(reject);
                        reject(error);
                        return;
                    }
                }
            }
            if fullfilled.lock().expect(LOCK_ERROR).len() == n_promises {
                let resolve = get_inner(resolve).expect(INNER_DOUBLE_RESOLVE_ERROR);
                resolve(get_inner(fullfilled));
            }
        })
    }

    #[allow(non_snake_case)]
    pub fn allSettled(
        promises: impl IntoIterator<Item = Promise<T, E>> + 'static,
    ) -> Promise<Vec<State<T, E>>, ()>
    where
        T: 'static,
        E: 'static,
    {
        // Last promise to resolve is responsible for resolving the all promise
        let results = Arc::new(Mutex::new(Some(Vec::new())));
        Promise::new(move |resolve, _reject| {
            let resolve = Arc::new(Mutex::new(Some(resolve)));

            let promises: Vec<_> = promises.into_iter().collect();
            let n_promises = promises.len();

            for promise in promises {
                let results_fullfilled = results.clone();
                let results_rejected = results.clone();

                let resolve_fullfilled = resolve.clone();
                let resolve_rejected = resolve.clone();

                {
                    let guard = promise.promise.clone();
                    let mut inner = guard.lock().expect(LOCK_ERROR);
                    if let State::Pending = inner.state {
                        inner.callback_fullfilled.replace(Box::new(move |value| {
                            let len;
                            {
                                let mut guard = results_fullfilled.lock().expect(LOCK_ERROR);
                                let results_ref = guard.as_mut().expect(INNER_DOUBLE_RESOLVE_ERROR);
                                results_ref.push(State::Fulfilled(value));
                                len = results_ref.len();
                            }

                            if len == n_promises {
                                let resolve = get_inner_multi(resolve_fullfilled);
                                resolve(get_inner_multi(results_fullfilled));
                            }
                        }));
                        inner.callback_rejected.replace(Box::new(move |error| {
                            let len;
                            {
                                let mut guard = results_rejected.lock().expect(LOCK_ERROR);
                                let results_ref = guard.as_mut().expect(INNER_DOUBLE_RESOLVE_ERROR);
                                results_ref.push(State::Rejected(error));
                                len = results_ref.len();
                            }

                            if len == n_promises {
                                let resolve = get_inner_multi(resolve_rejected);
                                resolve(get_inner_multi(results_rejected));
                            }
                        }));
                        continue;
                    }
                }

                let mut guard = results.lock().expect(LOCK_ERROR);
                let results = guard.as_mut().expect(INNER_DOUBLE_RESOLVE_ERROR);
                match get_inner(promise.promise).state {
                    State::Pending => unreachable!("Pending state is handled before"),
                    State::Fulfilled(value) => results.push(State::Fulfilled(value)),
                    State::Rejected(error) => results.push(State::Rejected(error)),
                }
            }
            if results
                .lock()
                .expect(LOCK_ERROR)
                .as_ref()
                .expect(INNER_DOUBLE_RESOLVE_ERROR)
                .len()
                == n_promises
            {
                let resolve = get_inner(resolve).expect(INNER_DOUBLE_RESOLVE_ERROR);
                resolve(get_inner(results).expect(INNER_DOUBLE_RESOLVE_ERROR));
            }
        })
    }

    pub fn any(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<T, Vec<E>>
    where
        T: 'static,
        E: 'static,
    {
        // Last promise to resolve is responsible for resolving the any promise
        let errors = Arc::new(Mutex::new(Vec::new()));
        Promise::new(move |resolve, reject| {
            let resolve = Arc::new(Mutex::new(Some(resolve)));
            let reject = Arc::new(Mutex::new(Some(reject)));
            let is_resolved = Arc::new(AtomicBool::new(false));

            let promises: Vec<_> = promises.into_iter().collect();
            let n_promises = promises.len();

            for promise in promises {
                let errors = errors.clone();

                let resolve = resolve.clone();
                let reject = reject.clone();

                let is_resolved_fullfilled = is_resolved.clone();
                let is_resolved_rejected = is_resolved.clone();

                {
                    let guard = promise.promise.clone();
                    let mut inner = guard.lock().expect(LOCK_ERROR);
                    if let State::Pending = inner.state {
                        inner.callback_fullfilled.replace(Box::new(move |value| {
                            if is_resolved_fullfilled.swap(true, Relaxed) {
                                return;
                            }

                            let resolve = get_inner_multi(resolve);
                            resolve(value);
                        }));
                        inner.callback_rejected.replace(Box::new(move |error| {
                            if is_resolved_rejected.load(Relaxed) {
                                return;
                            }

                            let mut errors_ref = errors.lock().expect(LOCK_ERROR);
                            errors_ref.push(error);

                            if errors_ref.len() == n_promises {
                                let reject = get_inner(reject).expect(INNER_DOUBLE_RESOLVE_ERROR);
                                drop(errors_ref);
                                reject(get_inner(errors));
                            }
                        }));
                        continue;
                    }
                }

                match get_inner(promise.promise).state {
                    State::Pending => unreachable!("Pending state is handled before"),
                    State::Fulfilled(value) => {
                        if is_resolved.swap(true, Relaxed) {
                            return;
                        }
                        let resolve = get_inner_multi(resolve);
                        resolve(value);
                        return;
                    }
                    State::Rejected(error) => {
                        if is_resolved.load(Relaxed) {
                            return;
                        }
                        let mut errors_ref = errors.lock().expect(LOCK_ERROR);
                        errors_ref.push(error);
                    }
                }
            }
            // No async promises, can resolve immediately
            if errors.lock().expect(LOCK_ERROR).len() == n_promises {
                let reject = get_inner(reject).expect(INNER_DOUBLE_RESOLVE_ERROR);
                reject(get_inner(errors));
            }
        })
    }

    pub fn race(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<T, E>
    where
        T: 'static,
        E: 'static,
    {
        Promise::new(move |resolve, reject| {
            let resolve = Arc::new(Mutex::new(Some(resolve)));
            let reject = Arc::new(Mutex::new(Some(reject)));
            let is_resolved = Arc::new(AtomicBool::new(false));

            for promise in promises {
                let resolve = resolve.clone();
                let reject = reject.clone();

                let is_resolved_fullfilled = is_resolved.clone();
                let is_finished_rejected = is_resolved_fullfilled.clone();

                {
                    let guard = promise.promise.clone();
                    let mut inner = guard.lock().expect(LOCK_ERROR);
                    if let State::Pending = inner.state {
                        inner.callback_fullfilled.replace(Box::new(move |value| {
                            if is_resolved_fullfilled.swap(true, Relaxed) {
                                return;
                            }

                            let resolve = get_inner_multi(resolve);
                            resolve(value);
                        }));
                        inner.callback_rejected.replace(Box::new(move |error| {
                            if is_finished_rejected.swap(true, Relaxed) {
                                return;
                            }

                            let reject = get_inner_multi(reject);
                            reject(error);
                        }));
                        continue;
                    }
                }

                match get_inner(promise.promise).state {
                    State::Pending => unreachable!("Pending state is handled before"),
                    State::Fulfilled(value) => {
                        if is_resolved.swap(true, Relaxed) {
                            return;
                        }
                        let resolve = get_inner_multi(resolve);
                        resolve(value);
                    }
                    State::Rejected(error) => {
                        if is_resolved.swap(true, Relaxed) {
                            return;
                        }
                        let reject = get_inner_multi(reject);
                        reject(error);
                    }
                }
                break;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn pending() {
        let promise: Promise<(), ()> = Promise::new(|_resolve, _reject| {
            // do  nothing
        });
        assert_eq!(promise.state(), State::Pending);
    }

    #[test]
    pub fn fulfilled() {
        let promise: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        assert_eq!(promise.state(), State::Fulfilled(42));
    }

    #[test]
    pub fn rejected() {
        let promise: Promise<(), &str> = Promise::new(|_resolve, reject| {
            reject("error");
        });
        assert_eq!(promise.state(), State::Rejected("error"));
    }

    #[test]
    pub fn fulfilled_on_timeout() {
        use std::thread;
        let promise: Promise<bool, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(true);
            });
        });
        assert_eq!(promise.state(), State::Pending);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Fulfilled(true));
    }

    #[test]
    #[should_panic]
    pub fn both_resolve_and_reject() {
        let _promise: Promise<(), ()> = Promise::new(|resolve, reject| {
            resolve(());
            reject(());
        });
    }

    #[test]
    pub fn then() {
        let promise: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2 = promise.then(|value| value * 2);
        assert_eq!(promise2.state(), State::Fulfilled(84));
    }

    #[test]
    pub fn then_on_rejected() {
        let promise: Promise<i32, &str> = Promise::new(|_resolve, reject| {
            reject("error");
        });
        let promise2 = promise.then(|value| value * 2);
        assert_eq!(promise2.state(), State::Rejected("error"));
    }

    #[test]
    pub fn then_on_timeout() {
        use std::thread;
        let promise: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(42);
            });
        });
        let promise2 = promise.then(|value| value * 2);
        assert_eq!(promise2.state(), State::Pending);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise2.state(), State::Fulfilled(84));
    }

    #[test]
    pub fn catch() {
        let promise: Promise<(), &str> = Promise::new(|_resolve, reject| {
            reject("error");
        });
        let promise2 = promise.catch(|error| format!("{}!", error));
        assert_eq!(promise2.state(), State::Rejected("error!".to_string()));
    }

    #[test]
    pub fn catch_on_fulfilled() {
        let promise: Promise<i32, &str> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2 = promise.catch(|error| format!("{}!", error));
        assert_eq!(promise2.state(), State::Fulfilled(42));
    }

    #[test]
    pub fn catch_on_timeout() {
        use std::thread;
        let promise: Promise<(), &str> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                reject("error");
            });
        });
        let promise2 = promise.catch(|error| format!("{}!", error));
        assert_eq!(promise2.state(), State::Pending);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise2.state(), State::Rejected("error!".to_string()));
    }

    #[test]
    pub fn finally() {
        let promise: Promise<i32, &str> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let outside = Arc::new(Mutex::new(false));
        let inside = outside.clone();
        let promise2 = promise.finally(move || {
            let mut outside = inside.lock().unwrap();
            *outside = true;
        });
        assert_eq!(promise2.state(), State::Fulfilled(42));
        assert_eq!(*outside.lock().unwrap(), true);
    }

    #[test]
    pub fn finally_on_rejected() {
        let promise: Promise<(), &str> = Promise::new(|_resolve, reject| {
            reject("error");
        });
        let outside = Arc::new(Mutex::new(false));
        let inside = outside.clone();
        let promise2 = promise.finally(move || {
            let mut outside = inside.lock().unwrap();
            *outside = true;
        });
        assert_eq!(promise2.state(), State::Rejected("error"));
        assert_eq!(*outside.lock().unwrap(), true);
    }

    #[test]
    pub fn finally_on_timeout() {
        use std::thread;
        let promise: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(42);
            });
        });
        let outside = Arc::new(Mutex::new(false));
        let inside = outside.clone();
        let promise2 = promise.finally(move || {
            let mut outside = inside.lock().unwrap();
            *outside = true;
        });
        assert_eq!(promise2.state(), State::Pending);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise2.state(), State::Fulfilled(42));
        assert_eq!(*outside.lock().unwrap(), true);
    }

    #[test]
    pub fn finally_on_timeout_rejected() {
        use std::thread;
        let promise: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                reject(42);
            });
        });
        let outside = Arc::new(Mutex::new(false));
        let inside = outside.clone();
        let promise2 = promise.finally(move || {
            let mut outside = inside.lock().unwrap();
            *outside = true;
        });
        assert_eq!(promise2.state(), State::Pending);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise2.state(), State::Rejected(42));
        assert_eq!(*outside.lock().unwrap(), true);
    }

    #[test]
    pub fn then_then() {
        let promise: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2 = promise.then(|value| value * 2);
        let promise3 = promise2.then(|value| value + 1);
        assert_eq!(promise3.state(), State::Fulfilled(85));
    }

    #[test]
    pub fn all() {
        let promise1: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise3: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::all(promises);
        assert_eq!(promise.state(), State::Fulfilled(vec![42, 42, 42]));
    }

    #[test]
    pub fn all_reject() {
        let promise1: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(0);
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(1);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::all(promises);
        assert_eq!(promise.state(), State::Rejected(0));
    }

    #[test]
    pub fn all_on_timeout() {
        use std::thread;
        let promise1: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(1);
        });
        let promise2: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                resolve(2);
            });
        });
        let promise3: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(3);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::all(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Fulfilled(vec![1, 2, 3]));
    }

    #[test]
    pub fn all_on_timeout_rejected() {
        use std::thread;
        let promise1: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            resolve(1);
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                reject(2);
            });
        });
        let promise3: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(3);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::all(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Rejected(2));
    }

    #[test]
    #[allow(non_snake_case)]
    pub fn allSettled() {
        let promise1: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise3: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::allSettled(promises);
        assert_eq!(
            promise.state(),
            State::Fulfilled(vec![
                State::Fulfilled(42),
                State::Fulfilled(42),
                State::Fulfilled(42)
            ])
        );
    }

    #[test]
    #[allow(non_snake_case)]
    pub fn allSettled_reject() {
        let promise1: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(0);
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(1);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::allSettled(promises);

        // Order of promises is not guaranteed
        let possible_results = vec![State::Fulfilled(42), State::Rejected(0), State::Rejected(1)];
        for promise in &promise.state().unwrap() {
            assert!(possible_results.contains(promise));
        }
    }

    #[test]
    #[allow(non_snake_case)]

    pub fn allSettled_on_timeout() {
        use std::thread;
        let promise1: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            resolve(1);
        });
        let promise2: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                resolve(2);
            });
        });
        let promise3: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(3);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::allSettled(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            promise.state(),
            State::Fulfilled(vec![
                State::Fulfilled(1),
                State::Fulfilled(2),
                State::Fulfilled(3)
            ])
        );
    }

    #[test]
    pub fn any() {
        let promise1: Promise<i32, i32> = Promise::new(|_resolve, _reject| {});
        let promise2: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(0);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::any(promises);
        assert_eq!(promise.state(), State::Fulfilled(42));
    }

    #[test]
    pub fn any_reject() {
        let promise1: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(42);
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(42);
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            reject(42);
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::any(promises);
        assert_eq!(promise.state(), State::Rejected(vec![42, 42, 42]));
    }

    #[test]
    pub fn any_on_timeout() {
        use std::thread;
        let promise1: Promise<i32, i32> = Promise::new(|_resolve, _reject| {});
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                reject(0);
            });
        });
        let promise3: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(42);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::any(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Fulfilled(42));
    }

    #[test]
    pub fn race() {
        use std::thread;
        let promise1: Promise<i32, i32> = Promise::new(|_resolve, _reject| {});
        let promise2: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            resolve(42);
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                reject(0);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::race(promises);
        assert_eq!(promise.state(), State::Fulfilled(42));
    }

    #[test]
    pub fn race_on_timeout() {
        use std::thread;
        let promise1: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(20));
                resolve(1);
            });
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                reject(0);
            });
        });
        let promise3: Promise<i32, i32> = Promise::new(|_resolve, _reject| {});
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::race(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Rejected(0));
    }
}
