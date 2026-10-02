//! A tiny abstraction so handlers can respond the same way to slash commands,
//! button presses and modal submissions.

use async_trait::async_trait;
use serenity::all::{
    CommandInteraction, ComponentInteraction, CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal,
    EditInteractionResponse, Message, ModalInteraction,
};
use serenity::http::Http;

#[async_trait]
pub trait Responder: Send + Sync {
    /// Respond with a message (use `.ephemeral(true)` on the builder for private replies).
    async fn reply(&self, http: &Http, msg: CreateInteractionResponseMessage) -> serenity::Result<()>;
    async fn show_modal(&self, http: &Http, modal: CreateModal) -> serenity::Result<()>;
    /// Acknowledge now, answer later with `edit`.
    async fn ack_ephemeral(&self, http: &Http) -> serenity::Result<()>;
    async fn edit(&self, http: &Http, edit: EditInteractionResponse) -> serenity::Result<Message>;
}

macro_rules! impl_responder {
    ($t:ty) => {
        #[async_trait]
        impl Responder for $t {
            async fn reply(&self, http: &Http, msg: CreateInteractionResponseMessage) -> serenity::Result<()> {
                self.create_response(http, CreateInteractionResponse::Message(msg))
                    .await
            }
            async fn show_modal(&self, http: &Http, modal: CreateModal) -> serenity::Result<()> {
                self.create_response(http, CreateInteractionResponse::Modal(modal))
                    .await
            }
            async fn ack_ephemeral(&self, http: &Http) -> serenity::Result<()> {
                self.defer_ephemeral(http).await
            }
            async fn edit(&self, http: &Http, edit: EditInteractionResponse) -> serenity::Result<Message> {
                self.edit_response(http, edit).await
            }
        }
    };
}

impl_responder!(CommandInteraction);
impl_responder!(ComponentInteraction);
impl_responder!(ModalInteraction);

/// Ephemeral text reply helper.
pub fn ephemeral(text: impl Into<String>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new().content(text).ephemeral(true)
}
