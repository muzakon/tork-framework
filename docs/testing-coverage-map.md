# Framework Coverage Map

This map tracks explicit test ownership for the framework-only crates:
`tork`, `tork-core`, `tork-macros`, and `tork-openapi`.

Coverage command:

```bash
framework/scripts/test-framework-coverage.sh
```

## Chapter Ownership

- `03-routing.md`
  - `crates/tork/tests/routing.rs`
  - `crates/tork-core/src/router/mod.rs`
  - `crates/tork-core/src/router/matcher.rs`
- `04-extractors-and-dependency-injection.md`
  - `crates/tork/tests/extractors.rs`
  - `crates/tork-core/src/extract/mod.rs`
  - `crates/tork-core/src/extract/body.rs`
  - `crates/tork-core/src/extract/header.rs`
  - `crates/tork-core/src/extract/path.rs`
- `05-models-and-validation.md`
  - `crates/tork/tests/validation.rs`
  - `crates/tork/tests/app_error.rs`
  - `crates/tork-core/src/extract/valid.rs`
- `06-responses-and-errors.md`
  - `crates/tork/tests/responses.rs`
  - `crates/tork/tests/errors.rs`
  - `crates/tork-core/src/error.rs`
  - `crates/tork-core/src/response/mod.rs`
  - `crates/tork-core/src/response/json.rs`
- `07-openapi-and-docs.md`
  - `crates/tork/tests/openapi.rs`
  - `crates/tork/tests/openapi_docs.rs`
  - `crates/tork-openapi/src/spec.rs`
  - `crates/tork-openapi/src/docs.rs`
  - `crates/tork-openapi/src/asyncapi.rs`
- `08-middleware.md`
  - `crates/tork/tests/middleware.rs`
  - `crates/tork-core/src/middleware/*.rs`
- `09-lifecycle-hooks-and-error-handling.md`
  - `crates/tork/tests/hooks.rs`
  - `crates/tork/tests/app_error.rs`
  - `crates/tork-core/src/app.rs`
- `10-server-sent-events.md`
  - `crates/tork/tests/sse.rs`
  - `crates/tork-core/src/testing/sse.rs`
- `11-websocket.md`
  - `crates/tork/tests/websocket.rs`
  - `crates/tork-core/src/testing/websocket.rs`
  - `crates/tork-openapi/src/asyncapi.rs`
- `12-forms-and-file-uploads.md`
  - `crates/tork/tests/forms.rs`
  - `crates/tork-core/src/multipart.rs`
- `13-settings.md`
  - `crates/tork/tests/settings.rs`
  - `crates/tork-core/src/settings.rs`
- `14-testing.md`
  - `crates/tork/tests/testing.rs`
  - `crates/tork-core/src/testing/request.rs`
  - `crates/tork-core/src/testing/response.rs`
  - `crates/tork-core/src/testing/cookie.rs`
  - `crates/tork-core/src/testing/sse.rs`
  - `crates/tork-core/src/testing/websocket.rs`
- `15-logging.md`
  - `crates/tork/tests/logging.rs`
  - `crates/tork-core/src/logging/*.rs`

## Notes

- Chapters `01`, `02`, and `16` are covered indirectly by public-facade smoke
  tests and the existing integration suites.
- ORM crates are intentionally excluded from this map and from the coverage run.
