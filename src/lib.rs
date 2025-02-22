//! # Promise Crate
//!
//! This crate provides a Rust-like implementation of JavaScript's Promise mechanism,
//! allowing asynchronous or delayed computations that can either be fulfilled with
//! a value or rejected with an error. It includes methods for chaining callbacks,
//! catching rejections, running final actions, and various combinators (like `all`
//! or `race`) to coordinate multiple promises.
//!
//! The interface is based on JavaScript's Promise, but adapted to Rust's ownership model.
//! For more information, see the [MDN Web Docs](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise).
//!
//! ## Usage
//!
//! To use this crate, create promises through [`Promise::new`], [`Promise::resolve`],
//! or [`Promise::reject`], then chain them with [`then`], [`catch`], or [`finally`].
//! For example:
//!
//! ```rust
//! use rsvow::{Promise, State};
//!
//! let promise: Promise<i32, ()> = Promise::resolve(42).then(|value| value * 2);
//! assert_eq!(promise.state(), State::Fulfilled(84));
//! ```
//!
//! For more complex cases, combinators like [`Promise::all`] and [`Promise::race`]
//! coordinate multiple tasks. See individual function docs for details.
use std::fmt::Debug;
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

/// Represents the current state of a promise.
///
/// A promise can be in one of three states:
///
/// - **Pending**: The promise is still in progress and has not yet settled.
/// - **Fulfilled(T)**: The promise has successfully completed with a value of type `T`.
/// - **Rejected(E)**: The promise has failed with an error of type `E`.
///
/// # Examples
///
/// Creating a fulfilled state:
///
/// ```rust
/// use rsvow::State;
///
/// let fulfilled: State<_, ()> = State::Fulfilled(42);
/// ```
///
/// Creating a rejected state:
///
/// ```rust
/// use rsvow::State;
///
/// let rejected: State<(), _> = State::Rejected("error");
/// ```
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum State<T, E> {
    Pending,
    Fulfilled(T),
    Rejected(E),
}

impl<T, E> State<T, E> {
    /// Returns the contained value if the state is Fulfilled.
    ///
    /// # Panics
    ///
    /// Panics if the state is not Fulfilled.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let state: State<i32, ()> = State::Fulfilled(42);
    /// assert_eq!(state.unwrap(), 42);
    /// ```
    pub fn unwrap(self) -> T {
        match self {
            State::Fulfilled(value) => value,
            _ => panic!("Invalid unwrap, state is not Fulfilled"),
        }
    }

    /// Returns the contained error if the state is Rejected.
    ///
    /// # Panics
    ///
    /// Panics if the state is not Rejected.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let state: State<i32, &str> = State::Rejected("error");
    /// assert_eq!(state.unwrap_err(), "error");
    /// ```
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

/// A `Promise` represents the eventual outcome of an asynchronous operation,
/// similar to JavaScript's Promise. It may be in one of three states:
///
/// - `State::Pending`: the operation is still in progress.
/// - `State::Fulfilled(T)`: the operation completed successfully with a value of type `T`.
/// - `State::Rejected(E)`: the operation failed with an error of type `E`.
///
/// # Creating Promises
///
/// A new promise is created using [`Promise::new`], which accepts an executor
/// function that receives `resolve` and `reject` callbacks to settle the promise.
/// Additionally, helper functions are provided:
///
/// - [`Promise::resolve`] returns a promise that is immediately fulfilled.
/// - [`Promise::reject`] returns a promise that is immediately rejected.
///
/// # Chaining and Error Handling
///
/// Once a promise is created, you can:
///
/// - Chain further operations with [`then`], which runs when the promise is fulfilled.
/// - Handle errors using [`catch`], which runs when the promise is rejected.
/// - Execute side effects regardless of the outcome using [`finally`].
///
/// # Combinators
///
/// For coordinating multiple promises, several methods are available:
///
/// - [`Promise::all`] waits until every promise fulfills (or rejects early if any reject),
/// - [`Promise::allSettled`] waits for all promises to settle and returns their states,
/// - [`Promise::any`] fulfills as soon as any promise fulfills (or rejects with all errors),
/// - [`Promise::race`] settles as soon as any promise settles.
///
/// # Example
///
/// ```rust
/// use rsvow::{Promise, State};
///
/// // Create a promise that resolves immediately with 42.
/// let promise: Promise<i32, ()> = Promise::resolve(42);
///
/// // Chain a `then` callback to double the value.
/// let doubled = promise.then(|value| value * 2);
///
/// // Check the result.
/// assert_eq!(doubled.state(), State::Fulfilled(84));
/// ```
///
/// This struct provides a flexible, chainable interface for asynchronous tasks,
/// enabling the composition of complex asynchronous workflows in a declarative style.
pub struct Promise<T, E> {
    promise: ArcMutex<PromiseInner<T, E>>,
}

impl<T: Debug, E: Debug> Debug for Promise<T, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Promise")
            .field("state", &self.promise.lock().expect(LOCK_ERROR).state)
            .finish()
    }
}

impl<T: PartialEq, E: PartialEq> PartialEq for Promise<T, E> {
    fn eq(&self, other: &Self) -> bool {
        self.promise.lock().expect(LOCK_ERROR).state
            == other.promise.lock().expect(LOCK_ERROR).state
    }
}

impl<T, E> Promise<T, E> {
    /// Creates a new Promise using the provided executor function.
    ///
    /// The executor receives `resolve` and `reject` closures to settle the promise.
    ///
    /// This is equivalent to the JavaScript Promise constructor.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let promise: Promise<i32, ()> = Promise::new(|resolve, _reject| {
    ///     resolve(42);
    /// });
    /// assert_eq!(promise.state(), State::Fulfilled(42));
    /// ```
    ///
    /// Multithreaded example:
    /// ```
    /// use rsvow::{Promise, State};
    /// use std::thread;
    ///
    /// let promise: Promise<u8, bool> = Promise::new(|_resolve, reject| {
    ///    thread::spawn(move || {
    ///       thread::sleep(std::time::Duration::from_millis(20));
    ///       reject(true);
    ///   });
    /// });
    /// assert_eq!(promise.state(), State::Pending);
    /// thread::sleep(std::time::Duration::from_millis(200));
    /// assert_eq!(promise.state(), State::Rejected(true));
    /// ```
    pub fn new<'a>(
        executor: impl FnOnce(Box<dyn FnOnce(T) + Send + 'a>, Box<dyn FnOnce(E) + Send + 'a>) + 'a,
    ) -> Self
    where
        T: Send + 'a,
        E: Send + 'a,
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

    /// Immediately resolves a promise with the given value.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let promise: Promise<i32, ()> = Promise::resolve(42);
    /// assert_eq!(promise.state(), State::Fulfilled(42));
    /// ```
    pub fn resolve(value: T) -> Self
    where
        T: Send,
        E: Send,
    {
        Self::new(|resolve, _reject| {
            resolve(value);
        })
    }

    /// Immediately rejects a promise with the given error.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    /// let promise: Promise<(), &str> = Promise::reject("error");
    /// assert_eq!(promise.state(), State::Rejected("error"));
    /// ```
    pub fn reject(error: E) -> Self
    where
        T: Send,
        E: Send,
    {
        Self::new(|_resolve, reject| {
            reject(error);
        })
    }

    /// Returns the current state of the promise.
    ///
    /// This intended to be used for testing and debugging purposes.
    /// No equivalent exists in JavaScript.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let promise: Promise<i32, ()> = Promise::resolve(-1);
    /// assert_eq!(promise.state(), State::Fulfilled(-1));
    /// ```
    pub fn state(&self) -> State<T, E>
    where
        T: Clone,
        E: Clone,
    {
        self.promise.lock().expect(LOCK_ERROR).state.clone()
    }

    /// Chains a callback executed when the promise is fulfilled.
    ///
    /// If the promise is fulfilled, `on_fulfilled` is called with the value.
    /// Otherwise, the rejection is passed through.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let promise: Promise<u64, String> = Promise::resolve(10).then(|v| v * 2);
    /// assert_eq!(promise.state(), State::Fulfilled(20));
    /// ```
    pub fn then<U: Send>(self, on_fulfilled: impl FnOnce(T) -> U + Send + 'static) -> Promise<U, E>
    where
        T: Send + 'static,
        U: Send + 'static,
        E: Send + 'static,
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

    /// Catches a rejection from the promise and transforms the error.
    ///
    /// If the promise is rejected, `on_rejected` is called with the error.
    /// Fulfilled promises pass the value along.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let promise: Promise<(), String> = Promise::reject("error").catch(|e| format!("{}!", e));
    /// assert_eq!(promise.state(), State::Rejected("error!".to_string()));
    /// ```
    pub fn catch<F: Send>(self, on_rejected: impl FnOnce(E) -> F + Send + 'static) -> Promise<T, F>
    where
        T: Send + 'static,
        F: Send + 'static,
        E: Send + 'static,
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

    /// Executes a callback when the promise is settled (fulfilled or rejected).
    ///
    /// The callback is executed regardless of the outcome.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    /// use std::sync::{Arc, Mutex};
    ///
    /// let flag = Arc::new(Mutex::new(false));
    /// let flag_clone = flag.clone();
    ///
    /// let promise: Promise<u16, ()> = Promise::resolve(42).finally(move || {
    ///     *flag_clone.lock().unwrap() = true;
    /// });
    /// assert_eq!(promise.state(), State::Fulfilled(42));
    /// assert_eq!(*flag.lock().unwrap(), true);
    /// ```
    pub fn finally(self, on_finally: impl FnOnce() + Send + 'static) -> Promise<T, E>
    where
        T: Send + 'static,
        E: Send + 'static,
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

    /// Waits for all promises to fulfill or rejects once any promise is rejected.
    ///
    /// Returns a promise that fulfills with a vector of all values or rejects immediately.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let p1 = Promise::resolve(1);
    /// let p2 = Promise::resolve(2);
    /// let p3 = Promise::resolve(3);
    ///
    /// let all: Promise<Vec<u8>, ()> = Promise::all(vec![p1, p2, p3]);
    /// match all.state() {
    ///    State::Fulfilled(mut values) => {
    ///       values.sort();
    ///       assert_eq!(values, vec![1, 2, 3]);
    ///   },
    ///  _ => unreachable!(),
    /// }
    /// ```
    pub fn all(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<Vec<T>, E>
    where
        T: Send + 'static,
        E: Send + 'static,
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

    /// Waits for all promises to settle and returns a promise with an array of their states.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let p1: Promise<i8, &str> = Promise::resolve(1);
    /// let p2: Promise<i8, &str> = Promise::reject("error");
    ///
    /// let all_settled = Promise::allSettled(vec![p1, p2]);
    /// match all_settled.state() {
    ///     State::Fulfilled(results) => {
    ///         assert!(results.iter().any(|r| matches!(r, State::Fulfilled(1))));
    ///         assert!(results.iter().any(|r| matches!(r, State::Rejected("error"))));
    ///     },
    ///     _ => unreachable!(),
    /// }
    /// ```
    #[allow(non_snake_case)]
    pub fn allSettled(
        promises: impl IntoIterator<Item = Promise<T, E>> + 'static,
    ) -> Promise<Vec<State<T, E>>, ()>
    where
        T: Send + 'static,
        E: Send + 'static,
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

    /// Resolves as soon as any promise is fulfilled, or rejects with a list of errors if all reject.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let p1 = Promise::reject(0);
    /// let p2 = Promise::resolve(42);
    /// let p3 = Promise::reject(1);
    ///
    /// let any: Promise<usize, Vec<i8>> = Promise::any(vec![p1, p2, p3]);
    /// assert_eq!(any.state(), State::Fulfilled(42));
    /// ```
    pub fn any(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<T, Vec<E>>
    where
        T: Send + 'static,
        E: Send + 'static,
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

    /// Settles as soon as any promise resolves or rejects, adopting that value.
    ///
    /// # Example
    ///
    /// ```
    /// use rsvow::{Promise, State};
    ///
    /// let p1: Promise<isize, ()> = Promise::new(|_resolve, _reject| {});
    /// let p2 = Promise::resolve(42);
    ///
    /// let race = Promise::race(vec![p1, p2]);
    /// assert_eq!(race.state(), State::Fulfilled(42));
    /// ```
    pub fn race(promises: impl IntoIterator<Item = Promise<T, E>> + 'static) -> Promise<T, E>
    where
        T: Send + 'static,
        E: Send + 'static,
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
