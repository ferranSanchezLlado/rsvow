# Rsvow

A Rust-like implementation of JavaScript's Promise mechanism.

## Overview

`rsvow` is a lightweight Rust crate that provides an API similar to JavaScript's Promise. It allows you to manage asynchronous or delayed computations with a familiar interface.

For more details, check out the [documentation](https://docs.rs/rsvow).

## Features

- **Simple API**: `rsvow` provides a simple and easy-to-use API that functions similarly to JavaScript's Promise.
- **Thread-safe**: `rsvow` is thread-safe and can be used in multi-threaded environments.
- **Asynchronous**: `rsvow` allows you to run code asynchronously and handle the results in a clean and concise way.
- **Error handling**: `rsvow` provides error handling mechanisms that allow you to catch and handle errors in a convenient manner.
- **Chaining**: `rsvow` supports chaining of promises, allowing you to create complex asynchronous workflows with ease.

## Installation

Add the following dependency to your `Cargo.toml`:

```toml
[dependencies]
rsvow = "0.1.0"
```

Or if you want to use the latest version from the main branch:
```toml
[dependencies]
rsvow = { git = "https://github.com/ferranSanchezLlado/rsvow.git" }
``` 

## Usage

Below is a simple example of using `rsvow`:

```rust
use rsvow::{Promise, State};

fn main() {
    // Create a promise that resolves immediately with a value.
    let promise: Promise<i32, ()> = Promise::resolve(42);

    // Chain a `then` callback to double the value.
    let doubled = promise.then(|value| value * 2);

    // Check the result.
    assert_eq!(doubled.state(), State::Fulfilled(84));
}
```

You can also run code asynchronously:

```rust
use rsvow::{Promise, State};
use std::thread;

fn main() {
    // Create a promise that immediately rejects.
    let promise: Promise<(), &str> = Promise::reject("error");

    // Transform the error message.
    let error_handled = promise.catch(|error| {
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(100));
            error(format!("Handled error: {}", error))
        })
    });

    assert_eq!(error_handled.state(), State::Pending);
    thread::sleep(std::time::Duration::from_millis(200));
    match error_handled.state() {
        State::Rejected(message) => println!("{}", message),
        _ => (),
    }
}
```

## License

Dual-licensed under MIT or Apache-2.0. See LICENSE for details.