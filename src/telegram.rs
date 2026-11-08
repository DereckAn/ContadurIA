//! Alertas por Telegram: vincular el chat del usuario y enviarle mensajes.

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::State, response::{IntoResponse, Redirect, Response}};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{AppError, AppState, auth::Usuario, var};

fn token() -> Result<String, AppError> {
    let t = var("TELEGRAM_TOKEN", "");
    if t.is_empty() {
        return Err(AppError::usuario("Falta configurar TELEGRAM_TOKEN."));
    }
    Ok(t)
}

/// Envía un mensaje a un chat. El texto admite HTML sencillo (<b>, <i>, <a>).
pub async fn enviar(http: &reqwest::Client, chat_id: &str, texto: &str) -> Result<(), AppError> {
    let token = token()?;
    let r = http
        .post(format!("https://api.telegram.org/bot{token}/sendMessage"))
        .json(&serde_json::json!({ "chat_id": chat_id, "text": texto, "parse_mode": "HTML" }))
        .send()
        .await?;
    if !r.status().is_success() {
        return Err(AppError::usuario("Telegram rechazó el mensaje. Vuelve a vincular tu chat."));
    }
    Ok(())
}

#[derive(Deserialize)]
struct Updates {
    result: Vec<Update>,
}
#[derive(Deserialize)]
struct Update {
    message: Option<Mensaje>,
}
#[derive(Deserialize)]
struct Mensaje {
    chat: Chat,
}
#[derive(Deserialize)]
struct Chat {
    id: i64,
}

/// Busca el chat más reciente que escribió al bot y lo guarda en el perfil.
pub async fn vincular(http: &reqwest::Client, db: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let token = token()?;
    let u: Updates = http
        .get(format!("https://api.telegram.org/bot{token}/getUpdates"))
        .send()
        .await?
        .json()
        .await?;
    let Some(chat) = u.result.iter().rev().find_map(|x| x.message.as_ref().map(|m| m.chat.id)) else {
        return Ok(false);
    };
    sqlx::query("UPDATE perfiles SET telegram_chat_id = $1 WHERE usuario_id = $2")
        .bind(chat.to_string())
        .bind(id)
        .execute(db)
        .await?;
    Ok(true)
}

async fn chat_de(db: &PgPool, id: Uuid) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar("SELECT telegram_chat_id FROM perfiles WHERE usuario_id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?
        .flatten())
}

// --- Handlers ---

#[derive(Template, WebTemplate)]
#[template(path = "telegram.html")]
pub struct Pagina {
    usuario_bot: String,
    conectado: bool,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Pagina, AppError> {
    Ok(Pagina {
        usuario_bot: var("TELEGRAM_BOT", "DereckAn_bot"),
        conectado: chat_de(&st.db, id).await?.is_some(),
    })
}

pub async fn vincular_handler(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    let ok = vincular(&st.http, &st.db, id).await?;
    if !ok {
        return Err(AppError::usuario("No encontré tu mensaje. Mándale /start al bot y vuelve a intentar."));
    }
    // Saludo de confirmación.
    if let Some(chat) = chat_de(&st.db, id).await? {
        let _ = enviar(&st.http, &chat, "✅ ContadurIA quedó conectado. Aquí te avisaré de tus obligaciones.").await;
    }
    Ok(Redirect::to("/telegram").into_response())
}

pub async fn probar_handler(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    let Some(chat) = chat_de(&st.db, id).await? else {
        return Err(AppError::usuario("Primero vincula tu Telegram."));
    };
    enviar(&st.http, &chat, "🔔 Mensaje de prueba de ContadurIA.").await?;
    Ok(Redirect::to("/telegram").into_response())
}
