//! Confirms the middleware surface is reachable through the facade crate.

use tork::{BoxFuture, DuplicatePolicy, Middleware, Next, Request, Response, Result};

// The `middleware` module is also reachable (built-ins land in later commits).
#[allow(unused_imports)]
use tork::middleware;

struct Noop;

impl Middleware for Noop {
    fn handle(&self, request: Request, next: Next) -> BoxFuture<'static, Result<Response>> {
        next.run(request)
    }

    fn duplicate_policy(&self) -> DuplicatePolicy {
        DuplicatePolicy::Allow
    }
}

#[test]
fn middleware_types_are_exported() {
    // Exercises that all of the core middleware types resolve through `tork`.
    assert!(!Noop.name().is_empty());
    assert_eq!(Noop.duplicate_policy(), DuplicatePolicy::Allow);
}
