# Tork Guide

Tork is a backend web framework for Rust, built directly on Hyper and Tokio. It
offers a high-level, declarative developer experience: annotation-based routers,
dependency injection, request validation, automatic OpenAPI, and middleware,
while staying close to the metal.

This guide walks through the framework from the ground up. Each chapter builds on
the previous one and ends with working code.

## Contents

1. [Introduction](01-introduction.md)
2. [Getting started](02-getting-started.md)
3. [Routing](03-routing.md)
4. [Extractors and dependency injection](04-extractors-and-dependency-injection.md)
5. [Models and validation](05-models-and-validation.md)
6. [Responses and errors](06-responses-and-errors.md)
7. [OpenAPI and docs](07-openapi-and-docs.md)
8. [Middleware](08-middleware.md)
9. [Lifecycle hooks and error handling](09-lifecycle-hooks-and-error-handling.md)
10. [Server-Sent Events](10-server-sent-events.md)
11. [WebSocket](11-websocket.md)
12. [Forms and file uploads](12-forms-and-file-uploads.md)
13. [Settings](13-settings.md)
14. [Project structure](14-project-structure.md)

## Status

The framework is built in phases. The runtime, routing, dependency injection,
validation and serialization, OpenAPI, the middleware layer, the lifecycle hooks
and error handling, Server-Sent Events, WebSocket, forms and file uploads, and
typed settings are in place. An ORM and a CLI are planned for later phases.

## Conventions in this guide

- Every code sample reflects the current public API.
- Examples use an application crate named `my_api`. Within this repository that
  crate depends on `tork` through a path dependency; a published application
  would write `tork = "0.1"` instead.
