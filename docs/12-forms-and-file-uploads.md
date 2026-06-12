# 12. Forms and file uploads

So far every request body has been JSON, read with `Valid<T>` or `Json<T>`. HTML
forms and file uploads use two other encodings: `application/x-www-form-urlencoded`
for plain key-value forms, and `multipart/form-data` for forms that carry files.
This chapter covers both, the two file types Tork offers, validation, and the
limits that keep uploads safe.

This chapter assumes you have read [Models and validation](05-models-and-validation.md).

## The two file types

A multipart file field binds to one of two types, depending on how large the file
is and what you do with it.

- `FileBytes` holds the whole file in memory as bytes. Use it for small files you
  read in one go: an avatar, a CSV, a thumbnail.
- `UploadFile` is spooled: it stays in memory up to a threshold, then spills to a
  temporary file on disk. Use it for large or unbounded uploads you stream to
  their final destination.

`FileBytes` exposes the bytes directly:

```rust
file.len();              // size in bytes
file.bytes();            // &[u8]
file.into_bytes();       // Bytes, by value
file.filename();         // Option<&str>, the client-supplied name
file.content_type();     // Option<&Mime>, the declared type
```

`UploadFile` carries metadata up front and reads its contents through async
methods, so the blocking disk IO never stalls the runtime:

```rust
file.filename();                 // Option<&str>
file.content_type();             // Option<&Mime>
file.size();                     // u64, total bytes

let bytes = file.read().await?;            // read it all
while let Some(chunk) = file.read_chunk(64 * 1024).await? { /* ... */ }
file.seek_start().await?;                   // rewind
file.save_to_dir("/var/uploads", "photo.jpg").await?;
```

`Mime` is re-exported as `tork::mime::Mime`, so you can name it without adding the
`mime` crate yourself.

## Two ways to read a form

Tork offers a parameter-based style for quick handlers and a model-based style for
anything you want validated and documented. They share the same field types and
validation; pick whichever reads better.

A handler declares exactly one body. Mixing a JSON body (`Valid`/`Json`) with form
or file parameters, or combining `Form` and `Multipart`, is a compile error.

## Parameter style

Annotate each parameter with `#[file]` for a file field or `#[form]` for a text
field. The field name defaults to the parameter name. The body is parsed once and
the fields are distributed to the parameters.

```rust
use tork::{post, FileBytes};

#[post("/files")]
pub async fn create_file(
    #[file] avatar: FileBytes,
    #[form] caption: String,
) -> tork::Result<FileInfo> {
    Ok(FileInfo { size: avatar.len(), caption })
}
```

A bare `FileBytes` or `UploadFile` parameter is treated as a file field even
without the attribute. Use `#[file(name = "...")]` or `#[form(name = "...")]` when
the form field name differs from the parameter.

Optional and repeated fields use `Option<T>` and `Vec<T>`:

```rust
#[post("/files")]
pub async fn upload(
    #[file] documents: Vec<UploadFile>,   // zero or more files under "documents"
    #[form] note: Option<String>,         // an optional text field
) -> tork::Result<()> {
    // ...
}
```

## Model style

For a real endpoint, describe the form once with `#[derive(FormModel)]` and take it
as `Multipart<T>`. Each field becomes a form field, validated and documented.

```rust
use tork::{post, FileBytes, UploadFile, FormModel, Multipart};

#[derive(FormModel)]
pub struct ProfileForm {
    #[file(max_size = "2MB", content_types = ["image/png", "image/jpeg"], sniff = true)]
    pub avatar: FileBytes,
    #[file(max_size = "20MB")]
    pub attachment: Option<UploadFile>,
    #[field(min_length = 2, max_length = 40)]
    pub display_name: String,
    pub tags: Vec<String>,
}

#[post("/profile")]
pub async fn save_profile(form: Multipart<ProfileForm>) -> tork::Result<ProfileOut> {
    let form = form.into_inner();
    // form.avatar, form.attachment, form.display_name, form.tags
    Ok(/* ... */)
}
```

`#[file(...)]` configures a file field and `#[field(...)]` adds the same text
constraints you know from [Models and validation](05-models-and-validation.md)
(`min_length`, `max_length`, `ge`, `le`, `gt`, `lt`). `Option<T>` fields are
optional; `Vec<T>` fields may repeat. A failed constraint returns `422
Unprocessable Entity` with a `VALIDATION_ERROR` body listing each problem.

## File validation

`#[file(...)]` accepts the following, on both a `FormModel` field and a `#[file]`
parameter:

- `max_size = "5MB"`: reject a file larger than this with `413 Payload Too Large`.
  Sizes accept `B`, `KB`, and `MB` suffixes.
- `content_types = ["image/png", "application/pdf"]`: allow only these declared
  content types; anything else returns `422`.
- `sniff = true`: inspect the leading bytes of the file and reject it when the
  real type does not match the declared one. This catches a GIF renamed to
  `.png`. It complements `content_types` rather than replacing it.

```rust
#[file(max_size = "5MB", content_types = ["image/png", "image/jpeg"], sniff = true)]
pub avatar: FileBytes,
```

## Urlencoded forms

A form without files is `application/x-www-form-urlencoded`. Read it with `Form<T>`,
where `T` is an ordinary `#[api_model]`. It is parsed and validated like a JSON
body, just from a different encoding.

```rust
use tork::{post, Form, api_model};

#[api_model]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

#[post("/login")]
pub async fn login(form: Form<LoginForm>) -> tork::Result<LoginOut> {
    let form = form.into_inner();
    // ...
}
```

## Upload limits

Uploads are bounded so a large or malicious request cannot exhaust memory or disk.
Set application-wide defaults with `App::upload_config`:

```rust
use tork::{App, UploadConfig};

App::new()
    .upload_config(
        UploadConfig::new()
            .max_body_size_mb(32)       // total request size
            .max_file_size_mb(16)       // per file
            .memory_threshold(1 << 20)  // spool to disk past 1 MB
            .max_files(16),
    )
    // ...
```

Override the limits for a single route with `upload(...)` on the route attribute.
Route values merge over the application defaults:

```rust
#[post("/upload", upload(max_file_size = "25MB", memory_threshold = "2MB"))]
pub async fn upload(form: Multipart<ProfileForm>) -> tork::Result<ProfileOut> {
    // ...
}
```

Exceeding `max_body_size` or a file's `max_file_size` returns `413 Payload Too
Large`; exceeding `max_files` is rejected the same way.

## OpenAPI

Form endpoints appear in the generated specification with the right media type.
A `Form<T>` body is documented as `application/x-www-form-urlencoded`, and a
`Multipart<T>` or parameter-based file handler as `multipart/form-data`, with file
fields marked `format: binary`:

```json
"requestBody": {
  "required": true,
  "content": {
    "multipart/form-data": {
      "schema": {
        "type": "object",
        "properties": {
          "avatar": { "type": "string", "format": "binary" },
          "display_name": { "type": "string" }
        },
        "required": ["avatar", "display_name"]
      }
    }
  }
}
```

See [OpenAPI and docs](07-openapi-and-docs.md) for how the document is served.
