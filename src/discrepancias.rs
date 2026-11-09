//! Detector de discrepancias con el SAT: depósitos sin respaldo (riesgo art. 91 LISR)
//! y CFDI cancelados que podrías estar contando.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Query, State};
use rust_decimal::Decimal;
use serde::Deserialize;
use time::{Date, macros::format_description};

use crate::{AppError, AppState, auth::Usuario};

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

#[derive(sqlx::FromRow)]
pub struct Deposito {
    fecha: Date,
    descripcion: String,
    abono: Decimal,
}
impl Deposito {
    pub fn fecha_texto(&self) -> String {
        self.fecha.format(format_description!("[day]/[month]/[year]")).unwrap_or_default()
    }
    pub fn monto_texto(&self) -> String {
        pesos(self.abono)
    }
}

#[derive(sqlx::FromRow)]
pub struct Cancelado {
    fecha: Date,
    emisor_nombre: String,
    total: Decimal,
    estado_sat: String,
}
impl Cancelado {
    pub fn fecha_texto(&self) -> String {
        self.fecha.format(format_description!("[day]/[month]/[year]")).unwrap_or_default()
    }
    pub fn total_texto(&self) -> String {
        pesos(self.total)
    }
    pub fn emisor(&self) -> &str {
        &self.emisor_nombre
    }
    pub fn estado(&self) -> &str {
        &self.estado_sat
    }
}

#[derive(Deserialize)]
pub struct Params {
    anio: Option<i32>,
}

#[derive(Template, WebTemplate)]
#[template(path = "discrepancias.html")]
pub struct Pagina {
    anio: i32,
    depositos_total: String,
    sin_respaldo_total: String,
    sin_respaldo: Vec<Deposito>,
    cancelados: Vec<Cancelado>,
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());

    let depositos_total: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(abono),0) FROM movimientos
         WHERE usuario_id = $1 AND abono IS NOT NULL AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_one(&st.db)
    .await?;

    // Depósitos sin CFDI que los respalde (posible ingreso no declarado).
    let sin_respaldo: Vec<Deposito> = sqlx::query_as(
        "SELECT fecha, descripcion, abono FROM movimientos
         WHERE usuario_id = $1 AND abono IS NOT NULL AND cfdi_uuid IS NULL
           AND EXTRACT(YEAR FROM fecha) = $2
         ORDER BY abono DESC",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;
    let sin_respaldo_total: Decimal = sin_respaldo.iter().map(|d| d.abono).sum();

    // CFDI que el SAT marca como cancelados (ya no deberías contarlos).
    let cancelados: Vec<Cancelado> = sqlx::query_as(
        "SELECT fecha::date AS fecha, emisor_nombre, total, estado_sat FROM cfdis
         WHERE usuario_id = $1 AND estado_sat IS NOT NULL AND estado_sat <> 'Vigente'
           AND EXTRACT(YEAR FROM fecha) = $2
         ORDER BY fecha DESC",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;

    Ok(Pagina {
        anio,
        depositos_total: pesos(depositos_total),
        sin_respaldo_total: pesos(sin_respaldo_total),
        sin_respaldo,
        cancelados,
    })
}
