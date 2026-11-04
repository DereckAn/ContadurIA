//! Perfil fiscal del usuario y página de inicio con su calendario.

use askama::Template;
use askama_web::WebTemplate;
use axum::{
    extract::State,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::Form;
use serde::Deserialize;
use sqlx::PgPool;
use time::{Date, macros::format_description};
use uuid::Uuid;

use crate::{
    AppError, AppState,
    auth::Usuario,
    calendario::{Obligacion, REGIMENES, obligaciones},
};

#[derive(sqlx::FromRow, Default)]
pub struct Perfil {
    nombre: String,
    rfc: String,
    regimenes: Vec<String>,
    efirma_vence: Option<Date>,
}

impl Perfil {
    pub fn tiene(&self, codigo: &str) -> bool {
        self.regimenes.iter().any(|r| r == codigo)
    }

    pub fn efirma_valor(&self) -> String {
        self.efirma_vence.map(|d| d.to_string()).unwrap_or_default()
    }
}

async fn cargar(db: &PgPool, id: Uuid) -> sqlx::Result<Option<Perfil>> {
    sqlx::query_as("SELECT nombre, rfc, regimenes, efirma_vence FROM perfiles WHERE usuario_id = $1")
        .bind(id)
        .fetch_optional(db)
        .await
}

#[derive(Template, WebTemplate)]
#[template(path = "inicio.html")]
pub struct Inicio {
    nombre: String,
    obligaciones: Vec<Obligacion>,
}

pub async fn inicio(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    let Some(p) = cargar(&st.db, id).await? else {
        return Ok(Redirect::to("/perfil").into_response());
    };
    let hoy = crate::ahora_mx().date();
    let obligaciones = obligaciones(&p.regimenes, p.efirma_vence, hoy, 90);
    Ok(Inicio { nombre: p.nombre, obligaciones }.into_response())
}

#[derive(Template, WebTemplate)]
#[template(path = "perfil.html")]
pub struct PerfilPagina {
    perfil: Perfil,
    regimenes: &'static [(&'static str, &'static str)],
    error: Option<String>,
}

pub async fn ver(State(st): State<AppState>, Usuario(id): Usuario) -> Result<PerfilPagina, AppError> {
    let perfil = cargar(&st.db, id).await?.unwrap_or_default();
    Ok(PerfilPagina { perfil, regimenes: REGIMENES, error: None })
}

#[derive(Deserialize)]
pub struct PerfilForm {
    nombre: String,
    rfc: String,
    #[serde(default)]
    regimenes: Vec<String>,
    efirma_vence: String,
}

pub async fn guardar(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Form(f): Form<PerfilForm>,
) -> Result<Response, AppError> {
    let perfil = Perfil {
        nombre: f.nombre.trim().to_owned(),
        rfc: f.rfc.trim().to_uppercase(),
        regimenes: f.regimenes,
        efirma_vence: Date::parse(f.efirma_vence.trim(), format_description!("[year]-[month]-[day]")).ok(),
    };
    if let Err(e) = validar(&perfil) {
        return Ok(PerfilPagina { perfil, regimenes: REGIMENES, error: Some(e.into()) }.into_response());
    }
    sqlx::query(
        "INSERT INTO perfiles (usuario_id, nombre, rfc, regimenes, efirma_vence) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (usuario_id) DO UPDATE SET nombre = EXCLUDED.nombre, rfc = EXCLUDED.rfc,
             regimenes = EXCLUDED.regimenes, efirma_vence = EXCLUDED.efirma_vence",
    )
    .bind(id)
    .bind(&perfil.nombre)
    .bind(&perfil.rfc)
    .bind(&perfil.regimenes)
    .bind(perfil.efirma_vence)
    .execute(&st.db)
    .await?;
    Ok(Redirect::to("/").into_response())
}

fn validar(p: &Perfil) -> Result<(), &'static str> {
    if p.nombre.is_empty() || p.nombre.len() > 200 {
        return Err("Escribe tu nombre.");
    }
    if !rfc_valido(&p.rfc) {
        return Err("El RFC de persona física tiene 13 caracteres: 4 letras, 6 dígitos de fecha y 3 de homoclave.");
    }
    if p.regimenes.is_empty() {
        return Err("Elige al menos un régimen fiscal.");
    }
    if !p.regimenes.iter().all(|r| REGIMENES.iter().any(|(c, _)| c == r)) {
        return Err("Régimen fiscal desconocido.");
    }
    Ok(())
}

/// Formato de RFC de persona física (no verifica que exista en el SAT).
fn rfc_valido(rfc: &str) -> bool {
    let c: Vec<char> = rfc.chars().collect();
    c.len() == 13
        && c[..4].iter().all(|ch| ch.is_ascii_uppercase() || *ch == 'Ñ' || *ch == '&')
        && c[4..10].iter().all(char::is_ascii_digit)
        && c[10..].iter().all(char::is_ascii_alphanumeric)
}

#[test]
fn rfc() {
    assert!(rfc_valido("PEÑA850101AB1"));
    assert!(!rfc_valido("PEPA850101AB")); // 12: persona moral
    assert!(!rfc_valido("PEPA85A101AB1"));
}
