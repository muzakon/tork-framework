//! Routes that demonstrate the hooks, error-handling, and streaming surface.

use tork::{
    FileBytes, Form, LastEventId, Multipart, RequestEvent, Sse, SseEvent, WebSocket, WsMessage,
    api_router, get, post, sse, websocket,
};

use crate::core::app_state::ChatHub;
use crate::core::errors::RepoError;
use crate::models::chat::{ChatIn, ChatMessage};
use crate::models::event::EventOut;
use crate::models::upload::{LoginForm, LoginOut, ProfileForm, UploadOut};

#[api_router(prefix = "/demo", tags = ["demo"])]
pub mod demo_router {
    use super::*;

    /// Surfaces a typed `RepoError`, mapped by the registered exception handler.
    ///
    /// Carries a route-level `on_request` hook (the named function below), so it
    /// runs in addition to the router-level audit hook.
    #[get("/db-error", on_request = note_db_call, summary = "Trigger a typed error")]
    pub async fn db_error() -> tork::Result<i64> {
        // The `?` converts `RepoError` into `tork::Error`, carrying the typed source.
        let outcome: Result<i64, RepoError> = Err(RepoError::Unavailable);
        Ok(outcome?)
    }

    /// Panics on purpose, to demonstrate the panic boundary.
    #[get("/panic", summary = "Trigger a handler panic")]
    pub async fn boom() -> tork::Result<i64> {
        panic!("demo panic");
    }

    /// Streams a few events over Server-Sent Events.
    ///
    /// Honors `Last-Event-ID` so a reconnecting client resumes after the last
    /// sequence number it saw.
    #[sse(
        "/stream",
        event = "tick",
        response_model = EventOut,
        summary = "Stream demo events"
    )]
    pub async fn stream(last: LastEventId) -> tork::Result<Sse<EventOut>> {
        let start = last
            .as_str()
            .and_then(|id| id.parse::<i64>().ok())
            .unwrap_or(0);

        let events = futures_util::stream::iter((start + 1..=start + 3).map(|sequence| {
            let out = EventOut {
                sequence,
                message: format!("event {sequence}"),
            };
            // Set the event id so a reconnecting client can resume from it.
            Ok::<_, tork::Error>(SseEvent::new(out).id(sequence))
        }));

        Ok(Sse::events(events))
    }

    /// Accepts a single file plus a caption as `multipart/form-data`, using the
    /// parameter-based style (`#[file]` / `#[form]`).
    #[post("/files", summary = "Upload a file (parameter style)")]
    pub async fn upload_file(
        #[file] avatar: FileBytes,
        #[form] caption: String,
    ) -> tork::Result<UploadOut> {
        Ok(UploadOut {
            avatar_size: avatar.len(),
            attachment_size: None,
            display_name: caption,
            tags: Vec::new(),
        })
    }

    /// Accepts a validated multipart form as a model (`Multipart<ProfileForm>`),
    /// with a larger upload limit set on the route.
    #[post("/upload", upload(max_file_size = "25MB"), summary = "Upload a profile (model style)")]
    pub async fn upload_profile(form: Multipart<ProfileForm>) -> tork::Result<UploadOut> {
        let form = form.into_inner();
        Ok(UploadOut {
            avatar_size: form.avatar.len(),
            attachment_size: form.attachment.as_ref().map(|file| file.size()),
            display_name: form.display_name,
            tags: form.tags,
        })
    }

    /// A urlencoded login form (`application/x-www-form-urlencoded`).
    #[post("/login", summary = "Log in with a urlencoded form")]
    pub async fn login(form: Form<LoginForm>) -> tork::Result<LoginOut> {
        let form = form.into_inner();
        Ok(LoginOut {
            username: form.username,
        })
    }

    /// Echoes every message back to the client over a WebSocket.
    #[websocket("/ws", summary = "Echo WebSocket")]
    pub async fn ws_echo(socket: WebSocket) -> tork::Result<()> {
        let mut socket = socket.accept().await?;
        while let Some(message) = socket.recv().await? {
            match message {
                WsMessage::Text(text) => socket.send_text(text).await?,
                WsMessage::Binary(bytes) => socket.send_binary(bytes).await?,
                WsMessage::Close(_) => break,
                _ => {}
            }
        }
        Ok(())
    }

    /// A typed broadcast chat room: validated input is fanned out to everyone in
    /// the room through the injected [`ChatHub`].
    #[websocket(
        "/chat/{room}",
        incoming = ChatIn,
        outgoing = ChatMessage,
        summary = "Broadcast chat room"
    )]
    pub async fn chat(socket: WebSocket, room: String, hub: ChatHub) -> tork::Result<()> {
        let mut socket = socket.accept().await?;
        let room = hub.0.room(room);
        let mut receiver = room.subscribe();
        loop {
            tokio::select! {
                incoming = socket.receive_valid::<ChatIn>() => match incoming? {
                    Some(input) => {
                        room.broadcast(ChatMessage {
                            from: "guest".to_owned(),
                            text: input.message,
                        });
                    }
                    None => break,
                },
                outgoing = receiver.recv() => match outgoing {
                    Ok(message) => socket.send_json(&message).await?,
                    Err(_) => break,
                },
            }
        }
        Ok(())
    }
}

/// Router-level audit hook: runs for every route in the demo router.
async fn audit_request(event: RequestEvent) {
    println!("audit: demo request {} {}", event.method(), event.path());
}

/// Route-level hook: runs only for the `/demo/db-error` route.
async fn note_db_call(event: RequestEvent) {
    println!("audit: db-error route touched on {}", event.path());
}

/// Builds the demo router with a router-scoped audit hook.
pub fn router() -> tork::Router {
    demo_router::router().on_request(audit_request)
}
