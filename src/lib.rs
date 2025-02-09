// Based on: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

type ArcMutex<T> = Arc<Mutex<T>>;

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
                if single_call_ensure_completion.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    panic!("Promise can only be resolved once");
                }

                let PromiseInner {
                    state,
                    callback_fullfilled,
                    ..
                } = &mut *promise_completion.lock().unwrap();
                match callback_fullfilled.take() {
                    Some(callback) => callback(value),
                    None => *state = State::Fulfilled(value),
                }
            }),
            Box::new(move |error| {
                if single_call_ensure_reject.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    panic!("Promise can only be resolved once");
                }

                let PromiseInner {
                    state,
                    callback_rejected,
                    ..
                } = &mut *promise_reject.lock().unwrap();
                match callback_rejected.take() {
                    Some(callback) => callback(error),
                    None => *state = State::Rejected(error),
                }
            }),
        );
        Self { promise }
    }

    pub fn state(&self) -> State<T, E>
    where
        T: Clone,
        E: Clone,
    {
        self.promise.lock().unwrap().state.clone()
    }

    pub fn then<U: Send>(self, on_fulfilled: impl FnOnce(T) -> U + Send + 'static) -> Promise<U, E>
    where
        T: 'static,
        U: 'static,
        E: 'static,
    {
        Promise::new(move |resolve, reject| {
            let on_fulfilled = move |value| {
                let new_value = on_fulfilled(value);
                resolve(new_value);
            };

            let guard = self.promise.clone();
            let mut inner = guard.lock().unwrap();
            if let State::Pending = inner.state {
                // Overwrite callback
                inner.callback_fullfilled.replace(Box::new(on_fulfilled));
                inner.callback_rejected.replace(Box::new(reject));
                return;
            }
            // Drops required to ensure only one strong reference to the promise
            drop(inner);
            drop(guard);

            let inner_state = Arc::try_unwrap(self.promise)
                .unwrap_or_else(|el| {
                    panic!(
                        "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                        Arc::strong_count(&el),
                        Arc::weak_count(&el)
                    )
                })
                .into_inner()
                .unwrap()
                .state;
            match inner_state {
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

            let guard = self.promise.clone();
            let mut inner = guard.lock().unwrap();
            if let State::Pending = inner.state {
                // Overwrite callback
                inner.callback_fullfilled.replace(Box::new(resolve));
                inner.callback_rejected.replace(Box::new(on_rejected));
                return;
            }
            // Drops required to ensure only one strong reference to the promise
            drop(inner);
            drop(guard);

            let inner_state = Arc::try_unwrap(self.promise)
                .unwrap_or_else(|el| {
                    panic!(
                        "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                        Arc::strong_count(&el),
                        Arc::weak_count(&el)
                    )
                })
                .into_inner()
                .unwrap()
                .state;
            match inner_state {
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
            let guard = self.promise.clone();
            let mut inner = guard.lock().unwrap();
            if let State::Pending = inner.state {
                // TODO: Find a way to avoid the Arc<Mutex<Option<FnOnce()>>> and the clone
                let on_finally_fullfilled = Arc::new(Mutex::new(Some(on_finally)));
                let on_finally_rejected = on_finally_fullfilled.clone();

                // Overwrite callback
                inner.callback_fullfilled.replace(Box::new(move |value| {
                    resolve(value);

                    let on_finally = on_finally_fullfilled.lock().unwrap().take().unwrap();
                    on_finally();
                }));
                inner.callback_rejected.replace(Box::new(move |error| {
                    reject(error);

                    let on_finally = on_finally_rejected.lock().unwrap().take().unwrap();
                    on_finally();
                }));
                return;
            }
            // Drops required to ensure only one strong reference to the promise
            drop(inner);
            drop(guard);

            let inner_state = Arc::try_unwrap(self.promise)
                .unwrap_or_else(|el| {
                    panic!(
                        "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                        Arc::strong_count(&el),
                        Arc::weak_count(&el)
                    )
                })
                .into_inner()
                .unwrap()
                .state;
            match inner_state {
                State::Pending => unreachable!("Pending state is handled before"),
                State::Fulfilled(value) => {
                    resolve(value);
                    on_finally();
                }
                State::Rejected(error) => {
                    reject(error);
                    on_finally();
                }
            }
        })
    }

    // Promise.all
    pub fn all(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<Vec<T>, E>
    where
        T: Send + 'static,
        E: Send + 'static,
    {
        let fullfilled = Arc::new(Mutex::new(Vec::new()));
        Promise::new(move |resolve, reject| {
            let resolve = Arc::new(Mutex::new(Some(resolve)));
            let reject = Arc::new(Mutex::new(Some(reject)));
            let is_rejected = Arc::new(AtomicBool::new(false));
            let mut is_pending = false;

            let promises: Vec<_> = promises.into_iter().collect();
            let n_promises = promises.len();

            for promise in promises {
                let fullfilled = fullfilled.clone();

                let resolve = resolve.clone();
                let reject = reject.clone();

                let is_rejected_fullfilled = is_rejected.clone();
                let is_rejected_reject = is_rejected.clone();

                let guard = promise.promise.clone();
                let mut inner = guard.lock().unwrap();
                if let State::Pending = inner.state {
                    is_pending = true;
                    // Overwrite callback
                    inner.callback_fullfilled.replace(Box::new(move |value| {
                        if is_rejected_fullfilled.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }

                        fullfilled.lock().unwrap().push(value);

                        // Check if all promises are resolved
                        if fullfilled.lock().unwrap().len() == n_promises {
                            let resolve = resolve.lock().unwrap().take().unwrap();

                            let inner_fullfilled = Arc::try_unwrap(fullfilled)
                                .unwrap_or_else(|el| {
                                    panic!(
                                        "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                                        Arc::strong_count(&el),
                                        Arc::weak_count(&el)
                                    )
                                })
                                .into_inner()
                                .unwrap();
                            resolve(inner_fullfilled);
                        }
                    }));
                    inner.callback_rejected.replace(Box::new(move |error| {
                        if is_rejected_reject.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }

                        let reject = reject.lock().unwrap().take().unwrap();
                        is_rejected_reject.store(true, std::sync::atomic::Ordering::Relaxed);
                        reject(error);
                    }));
                    continue;
                }
                // Drops required to ensure only one strong reference to the promise
                drop(inner);
                drop(guard);

                let inner_state = Arc::try_unwrap(promise.promise)
                    .unwrap_or_else(|el| {
                        panic!(
                            "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                            Arc::strong_count(&el),
                            Arc::weak_count(&el)
                        )
                    })
                    .into_inner()
                    .unwrap()
                    .state;
                match inner_state {
                    State::Pending => unreachable!("Pending state is handled before"),
                    State::Fulfilled(value) => fullfilled.lock().unwrap().push(value),
                    State::Rejected(error) => {
                        let reject = reject.lock().unwrap().take().unwrap();
                        reject(error);
                        return;
                    }
                }
            }
            // No async promises, can resolve immediately
            if !is_pending {
                let resolve = resolve.lock().unwrap().take().unwrap();
                let inner_fullfilled = Arc::try_unwrap(fullfilled)
                    .unwrap_or_else(|el| {
                        panic!(
                            "Failed to unwrap Arc. Should not happen (strong={}, weak={})",
                            Arc::strong_count(&el),
                            Arc::weak_count(&el)
                        )
                    })
                    .into_inner()
                    .unwrap();
                resolve(inner_fullfilled);
            }
        })
    }
    // Promise.allSettled
    // Promise.any
    // Promise.resolve
    // Promise.reject
    // Promise.race
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
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // reject after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // reject after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // resolve after 1 second
            // create a new thread
            resolve(1);
        });
        let promise2: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                resolve(2);
            });
        });
        let promise3: Promise<i32, ()> = Promise::new(|resolve, _reject| {
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
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
            // resolve after 1 second
            // create a new thread
            resolve(1);
        });
        let promise2: Promise<i32, i32> = Promise::new(|_resolve, reject| {
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(10));
                reject(2);
            });
        });
        let promise3: Promise<i32, i32> = Promise::new(|resolve, _reject| {
            // resolve after 1 second
            // create a new thread
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_millis(100));
                resolve(3);
            });
        });
        let promises: Vec<Promise<_, _>> = vec![promise1, promise2, promise3];
        let promise = Promise::all(promises);
        thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(promise.state(), State::Rejected(2));
    }
}
