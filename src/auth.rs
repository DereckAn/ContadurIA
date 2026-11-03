//! Cuentas con passkeys: sin contraseñas. El navegador crea y usa la llave;
//! el servidor sólo guarda la parte pública.

use askama::Template;
use askama_web::WebTemplate;
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use sqlx::types::Json as Jsonb;
use tower_sessions::Session;
use webauthn_rs::prelude::*;

use crate::{AppError, AppState};

const USUARIO: &str = "usuario_id";
const REGISTRO: &str = "registro_pendiente";
const ACCESO: &str = "acceso_pendiente";

/// Usuario con sesión iniciada. Si no hay sesión, redirige a /entrar.
pub struct Usuario(pub Uuid);

impl<S: Send + Sync> FromRequestParts<S> for Usuario {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Response> {
        let sesion = Session::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        match sesion.get::<Uuid>(USUARIO).await.ok().flatten() {
            Some(id) => Ok(Usuario(id)),
            None => Err(Redirect::to("/entrar").into_response()),
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "entrar.html")]
pub struct Entrar;

pub async fn pagina() -> Entrar {
    Entrar
}

#[derive(Deserialize)]
pub struct Correo {
    correo: String,
}

fn normalizar(correo: &str) -> Result<String, AppError> {
    let c = correo.trim().to_lowercase();
    if c.len() > 254 || !c.contains('@') {
        return Err(AppError::usuario("Escribe un correo válido."));
    }
    Ok(c)
}

pub async fn registro_inicio(
    State(st): State<AppState>,
    sesion: Session,
    Json(f): Json<Correo>,
) -> Result<Json<CreationChallengeResponse>, AppError> {
    let correo = normalizar(&f.correo)?;
    let existe: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM usuarios WHERE correo = $1)")
        .bind(&correo)
        .fetch_one(&st.db)
        .await?;
    if existe {
        return Err(AppError(StatusCode::CONFLICT, "Ese correo ya tiene cuenta; usa Entrar.".into()));
    }
    let id = Uuid::new_v4();
    let (reto, estado) = st.webauthn.start_passkey_registration(id, &correo, &correo, None)?;
    sesion.insert(REGISTRO, (id, correo, estado)).await?;
    Ok(Json(reto))
}

pub async fn registro_fin(
    State(st): State<AppState>,
    sesion: Session,
    Json(cred): Json<RegisterPublicKeyCredential>,
) -> Result<StatusCode, AppError> {
    let Some((id, correo, estado)) = sesion.remove::<(Uuid, String, PasskeyRegistration)>(REGISTRO).await? else {
        return Err(AppError::usuario("La solicitud expiró; intenta de nuevo."));
    };
    let passkey = st
        .webauthn
        .finish_passkey_registration(&cred, &estado)
        .map_err(|_| AppError::usuario("No se pudo verificar la passkey."))?;

    let mut tx = st.db.begin().await?;
    sqlx::query("INSERT INTO usuarios (id, correo) VALUES ($1, $2)")
        .bind(id)
        .bind(&correo)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO passkeys (usuario_id, datos) VALUES ($1, $2)")
        .bind(id)
        .bind(Jsonb(&passkey))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    iniciar_sesion(&sesion, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn acceso_inicio(
    State(st): State<AppState>,
    sesion: Session,
    Json(f): Json<Correo>,
) -> Result<Json<RequestChallengeResponse>, AppError> {
    let correo = normalizar(&f.correo)?;
    let filas: Vec<(Uuid, Jsonb<Passkey>)> = sqlx::query_as(
        "SELECT u.id, p.datos FROM usuarios u JOIN passkeys p ON p.usuario_id = u.id WHERE u.correo = $1",
    )
    .bind(&correo)
    .fetch_all(&st.db)
    .await?;
    // ponytail: revela si el correo existe; aceptable en beta, unificar mensajes antes de abrir al público.
    let Some(id) = filas.first().map(|f| f.0) else {
        return Err(AppError::usuario("No hay cuenta con ese correo; usa Crear cuenta."));
    };
    let passkeys: Vec<Passkey> = filas.into_iter().map(|f| f.1.0).collect();
    let (reto, estado) = st.webauthn.start_passkey_authentication(&passkeys)?;
    sesion.insert(ACCESO, (id, estado)).await?;
    Ok(Json(reto))
}

pub async fn acceso_fin(
    State(st): State<AppState>,
    sesion: Session,
    Json(cred): Json<PublicKeyCredential>,
) -> Result<StatusCode, AppError> {
    let Some((id, estado)) = sesion.remove::<(Uuid, PasskeyAuthentication)>(ACCESO).await? else {
        return Err(AppError::usuario("La solicitud expiró; intenta de nuevo."));
    };
    let resultado = st
        .webauthn
        .finish_passkey_authentication(&cred, &estado)
        .map_err(|_| AppError::usuario("No se pudo verificar la passkey."))?;

    // Actualiza el contador de la passkey usada (detecta llaves clonadas).
    let filas: Vec<(i64, Jsonb<Passkey>)> = sqlx::query_as("SELECT id, datos FROM passkeys WHERE usuario_id = $1")
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    for (pk_id, Jsonb(mut pk)) in filas {
        if pk.update_credential(&resultado) == Some(true) {
            sqlx::query("UPDATE passkeys SET datos = $1 WHERE id = $2")
                .bind(Jsonb(&pk))
                .bind(pk_id)
                .execute(&st.db)
                .await?;
        }
    }

    iniciar_sesion(&sesion, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn iniciar_sesion(sesion: &Session, id: Uuid) -> Result<(), AppError> {
    sesion.cycle_id().await?; // id de sesión nuevo al entrar: evita fijación de sesión
    sesion.insert(USUARIO, id).await?;
    Ok(())
}

pub async fn salir(sesion: Session) -> Result<Redirect, AppError> {
    sesion.flush().await?;
    Ok(Redirect::to("/entrar"))
}
