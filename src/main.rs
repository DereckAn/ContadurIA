mod anual;
mod auth;
mod banco;
mod calendario;
mod cfdi;
mod fiscal;
mod informe;
mod iva;
mod perfil;
mod resumen;
mod sat;
mod tareas;
mod telegram;

use std::sync::Arc;

use axum::{
    Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use sqlx::PgPool;
use time::{OffsetDateTime, PrimitiveDateTime, macros::offset};
use tower_http::services::ServeDir;
use tower_sessions::{Expiry, SessionManagerLayer, cookie::time::Duration, session_store::ExpiredDeletion};
use tower_sessions_sqlx_store::PostgresStore;
use webauthn_rs::prelude::{Url, Webauthn, WebauthnBuilder};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub webauthn: Arc<Webauthn>,
    pub http: reqwest::Client,
}

/// Fecha y hora actuales en el centro de México, sin fracciones de segundo.
// ponytail: UTC-6 fijo (sin horario de verano desde 2022); zona por usuario si hay usuarios en otros husos.
pub fn ahora_mx() -> PrimitiveDateTime {
    let t = OffsetDateTime::now_utc().to_offset(offset!(-6));
    PrimitiveDateTime::new(t.date(), t.time().replace_nanosecond(0).unwrap())
}

/// Error de un handler: el mensaje se muestra al usuario tal cual.
pub struct AppError(pub StatusCode, pub String);

impl AppError {
    pub fn usuario(msg: &str) -> Self {
        AppError(StatusCode::BAD_REQUEST, msg.into())
    }
}

/// Cualquier error inesperado (base de datos, sesión...) se registra y se oculta al usuario.
impl<E: std::fmt::Display> From<E> for AppError {
    fn from(e: E) -> Self {
        eprintln!("error: {e}");
        AppError(StatusCode::INTERNAL_SERVER_ERROR, "Error interno, intenta de nuevo.".into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

pub fn var(nombre: &str, defecto: &str) -> String {
    std::env::var(nombre).unwrap_or_else(|_| defecto.into())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok(); // carga variables de .env si existe
    let app_url = var("APP_URL", "http://localhost:3000");
    let origen = Url::parse(&app_url).expect("APP_URL no es una URL válida");
    let rp_id = origen.domain().expect("APP_URL necesita un dominio").to_owned();
    let webauthn = WebauthnBuilder::new(&rp_id, &origen)
        .and_then(|b| b.rp_name("ContadurIA").build())
        .expect("configuración de passkeys inválida");

    let db = PgPool::connect(&var("DATABASE_URL", "postgres://contaduria:contaduria@localhost/contaduria"))
        .await
        .expect("no se pudo conectar a Postgres");
    sqlx::migrate!().run(&db).await.expect("falló una migración");

    let sesiones = PostgresStore::new(db.clone());
    sesiones.migrate().await.expect("no se pudo crear la tabla de sesiones");
    tokio::spawn(sesiones.clone().continuously_delete_expired(tokio::time::Duration::from_secs(3600)));

    let estado = AppState {
        db,
        webauthn: Arc::new(webauthn),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("cliente HTTP"),
    };
    tokio::spawn(tareas::programador(estado.clone()));

    let app = Router::new()
        .route("/", get(perfil::inicio))
        .route("/perfil", get(perfil::ver).post(perfil::guardar))
        .route("/entrar", get(auth::pagina))
        .route("/salir", post(auth::salir))
        .route("/auth/registro/inicio", post(auth::registro_inicio))
        .route("/auth/registro/fin", post(auth::registro_fin))
        .route("/auth/acceso/inicio", post(auth::acceso_inicio))
        .route("/auth/acceso/fin", post(auth::acceso_fin))
        .route("/resumen", get(resumen::pagina))
        .route("/anual", get(anual::pagina))
        .route("/anual/expediente", get(anual::expediente))
        .route("/informe", get(informe::pagina))
        .route("/informe/enviar", post(informe::enviar_handler))
        .route("/telegram", get(telegram::pagina))
        .route("/telegram/vincular", post(telegram::vincular_handler))
        .route("/telegram/probar", post(telegram::probar_handler))
        .route("/banco", get(banco::pagina))
        .route("/banco/subir", post(banco::subir))
        .route("/banco/conciliar", post(banco::conciliar_handler))
        .route("/iva", get(iva::pagina))
        .route("/cfdi", get(cfdi::pagina))
        .route("/cfdi/validar", post(cfdi::validar))
        .route("/sat", get(sat::pagina))
        .route("/sat/pendientes", get(sat::pendientes))
        .route("/sat/enviar", post(sat::enviar))
        .nest_service("/static", ServeDir::new("static"))
        .layer(
            SessionManagerLayer::new(sesiones)
                .with_secure(origen.scheme() == "https")
                .with_expiry(Expiry::OnInactivity(Duration::days(7))),
        )
        .with_state(estado);

    let direccion = var("BIND", "127.0.0.1:3000");
    let listener = tokio::net::TcpListener::bind(&direccion).await.expect("no se pudo abrir el puerto");
    println!("ContadurIA en {app_url}");
    axum::serve(listener, app).await.unwrap();
}
