//! Cuentas con passkeys: sin contraseñas. El navegador crea y usa la llave;
//! el servidor sólo guarda la parte pública.

use askama::Template;
use askama_web::WebTemplate;
use axum::{
    Json,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::types::Json as Jsonb;
use time::OffsetDateTime;
use tower_sessions::Session;
use webauthn_rs::prelude::*;

use crate::{AppError, AppState};

const USUARIO: &str = "usuario_id";
const REGISTRO: &str = "registro_pendiente";
const ACCESO: &str = "acceso_pendiente";
const AGREGAR: &str = "agregar_pendiente";
/// Usuario que abrió un enlace de recuperación (todavía sin sesión) y hasta cuándo (unix).
const RECUPERACION: &str = "recuperacion";
const MINUTOS_RECUPERACION: i64 = 15;

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

// ---------- Seguridad: listar, agregar y quitar passkeys ----------

#[derive(sqlx::FromRow)]
pub struct Llave {
    id: i64,
    creada: OffsetDateTime,
}

impl Llave {
    pub fn fecha(&self) -> String {
        let f = self.creada.to_offset(time::macros::offset!(-6));
        format!("{:02}/{:02}/{}", f.day(), f.month() as u8, f.year())
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "seguridad.html")]
pub struct Seguridad {
    correo: String,
    llaves: Vec<Llave>,
}

pub async fn seguridad(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Seguridad, AppError> {
    let correo = sqlx::query_scalar("SELECT correo FROM usuarios WHERE id = $1").bind(id).fetch_one(&st.db).await?;
    let llaves = sqlx::query_as("SELECT id, creada FROM passkeys WHERE usuario_id = $1 ORDER BY creada")
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    Ok(Seguridad { correo, llaves })
}

/// A quién se le agrega la passkey: al de la sesión, o al que abrió un enlace de recuperación
/// hace menos de 15 minutos.
async fn usuario_para_agregar(sesion: &Session) -> Result<Uuid, AppError> {
    if let Some(id) = sesion.get::<Uuid>(USUARIO).await? {
        return Ok(id);
    }
    match sesion.get::<(Uuid, i64)>(RECUPERACION).await? {
        Some((id, hasta)) if OffsetDateTime::now_utc().unix_timestamp() < hasta => Ok(id),
        _ => Err(AppError(StatusCode::UNAUTHORIZED, "El enlace expiró; pide otro.".into())),
    }
}

pub async fn agregar_inicio(
    State(st): State<AppState>,
    sesion: Session,
) -> Result<Json<CreationChallengeResponse>, AppError> {
    let id = usuario_para_agregar(&sesion).await?;
    let correo: String = sqlx::query_scalar("SELECT correo FROM usuarios WHERE id = $1").bind(id).fetch_one(&st.db).await?;
    // Excluye las llaves que ya tiene: el navegador avisa si este dispositivo ya está registrado.
    let existentes: Vec<Jsonb<Passkey>> = sqlx::query_scalar("SELECT datos FROM passkeys WHERE usuario_id = $1")
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    let excluir = existentes.iter().map(|p| p.0.cred_id().clone()).collect();
    let (reto, estado) = st.webauthn.start_passkey_registration(id, &correo, &correo, Some(excluir))?;
    sesion.insert(AGREGAR, (id, estado)).await?;
    Ok(Json(reto))
}

pub async fn agregar_fin(
    State(st): State<AppState>,
    sesion: Session,
    Json(cred): Json<RegisterPublicKeyCredential>,
) -> Result<StatusCode, AppError> {
    let Some((id, estado)) = sesion.remove::<(Uuid, PasskeyRegistration)>(AGREGAR).await? else {
        return Err(AppError::usuario("La solicitud expiró; intenta de nuevo."));
    };
    let passkey = st
        .webauthn
        .finish_passkey_registration(&cred, &estado)
        .map_err(|_| AppError::usuario("No se pudo verificar la passkey."))?;
    sqlx::query("INSERT INTO passkeys (usuario_id, datos) VALUES ($1, $2)")
        .bind(id)
        .bind(Jsonb(&passkey))
        .execute(&st.db)
        .await?;
    // Si venía de un enlace de recuperación, ya puede entrar.
    if sesion.remove::<(Uuid, i64)>(RECUPERACION).await?.is_some() {
        iniciar_sesion(&sesion, id).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Quitar {
    id: i64,
}

/// Quita una passkey, salvo la última: sin ninguna, la cuenta quedaría inaccesible.
pub async fn quitar(State(st): State<AppState>, Usuario(id): Usuario, Form(f): Form<Quitar>) -> Result<Redirect, AppError> {
    sqlx::query(
        "DELETE FROM passkeys WHERE id = $1 AND usuario_id = $2
           AND (SELECT count(*) FROM passkeys WHERE usuario_id = $2) > 1",
    )
    .bind(f.id)
    .bind(id)
    .execute(&st.db)
    .await?;
    Ok(Redirect::to("/seguridad"))
}

// ---------- Recuperación por correo ----------

#[derive(Template, WebTemplate)]
#[template(path = "recuperar.html")]
pub struct Recuperar {
    /// "pedir", "enviado", "crear" o "invalido".
    estado: &'static str,
}

pub async fn recuperar_pagina() -> Recuperar {
    Recuperar { estado: "pedir" }
}

/// Manda el enlace de recuperación. Responde igual exista o no la cuenta, para no revelar correos.
pub async fn recuperar_pedir(State(st): State<AppState>, Form(f): Form<Correo>) -> Result<Recuperar, AppError> {
    let correo = normalizar(&f.correo)?;
    sqlx::query("DELETE FROM recuperaciones WHERE expira < now()").execute(&st.db).await?;
    let usuario: Option<Uuid> = sqlx::query_scalar("SELECT id FROM usuarios WHERE correo = $1")
        .bind(&correo)
        .fetch_optional(&st.db)
        .await?;
    if let Some(id) = usuario {
        // 244 bits aleatorios del sistema (dos UUID v4). Se guarda sólo su sha256.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        // Un solo enlace vivo por cuenta: el nuevo invalida los anteriores.
        sqlx::query("DELETE FROM recuperaciones WHERE usuario_id = $1").bind(id).execute(&st.db).await?;
        sqlx::query(
            "INSERT INTO recuperaciones (token_hash, usuario_id, expira)
             VALUES (encode(sha256(convert_to($1, 'UTF8')), 'hex'), $2, now() + make_interval(mins => $3))",
        )
        .bind(&token)
        .bind(id)
        .bind(MINUTOS_RECUPERACION as i32)
        .execute(&st.db)
        .await?;
        let enlace = format!("{}/recuperar/{token}", crate::var("APP_URL", "http://localhost:3000"));
        if crate::informe::hay_correo() {
            let html = format!(
                "<p>Alguien (ojalá tú) pidió recuperar tu cuenta de ContadurIA.</p>\
                 <p><a href=\"{enlace}\">Crear una passkey nueva</a> (vale {MINUTOS_RECUPERACION} minutos, una sola vez).</p>\
                 <p>Si no fuiste tú, ignora este correo.</p>"
            );
            if let Err(AppError(_, e)) = crate::informe::enviar_correo(&correo, "Recupera tu cuenta de ContadurIA", html).await {
                eprintln!("recuperación: no se pudo enviar el correo: {e}");
            }
        } else {
            // ponytail: sin SMTP (desarrollo local) el enlace sale en la terminal del servidor.
            // En producción configura SMTP_*; nunca expongas esta salida.
            eprintln!("recuperación (sin SMTP configurado) para {correo}: {enlace}");
        }
    }
    Ok(Recuperar { estado: "enviado" })
}

/// Abre el enlace: lo consume (sirve una sola vez) y deja crear una passkey nueva.
pub async fn recuperar_abrir(
    State(st): State<AppState>,
    sesion: Session,
    Path(token): Path<String>,
) -> Result<Recuperar, AppError> {
    let usuario: Option<Uuid> = sqlx::query_scalar(
        "DELETE FROM recuperaciones
         WHERE token_hash = encode(sha256(convert_to($1, 'UTF8')), 'hex') AND expira > now()
         RETURNING usuario_id",
    )
    .bind(&token)
    .fetch_optional(&st.db)
    .await?;
    let Some(id) = usuario else {
        return Ok(Recuperar { estado: "invalido" });
    };
    sesion.cycle_id().await?;
    let hasta = OffsetDateTime::now_utc().unix_timestamp() + MINUTOS_RECUPERACION * 60;
    sesion.insert(RECUPERACION, (id, hasta)).await?;
    Ok(Recuperar { estado: "crear" })
}
