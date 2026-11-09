//! IVA mensual: trasladado (lo que cobraste) menos acreditable (lo que pagaste).

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Query, State};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{AppError, AppState, auth::Usuario};

const MESES: [&str; 12] = [
    "Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio",
    "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre",
];

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

#[derive(sqlx::FromRow)]
pub struct Mes {
    mes: i32,
    trasladado: Decimal,
    acreditable: Decimal,
}

impl Mes {
    pub fn nombre(&self) -> &'static str {
        MESES.get((self.mes - 1) as usize).copied().unwrap_or("?")
    }
    pub fn trasladado_texto(&self) -> String { pesos(self.trasladado) }
    pub fn acreditable_texto(&self) -> String { pesos(self.acreditable) }
    /// Positivo = a pagar; negativo = a favor.
    pub fn diferencia_texto(&self) -> String { pesos(self.trasladado - self.acreditable) }
}

#[derive(Deserialize)]
pub struct Params {
    anio: Option<i32>,
}

#[derive(Template, WebTemplate)]
#[template(path = "iva.html")]
pub struct Pagina {
    anio: i32,
    meses: Vec<Mes>,
    trasladado: String,
    acreditable: String,
    diferencia: String,
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());
    // IVA = total - subtotal de CFDI de ingreso (tipo I). Lo "sin efectos fiscales" (S01) no es acreditable.
    // ponytail: aproxima IVA como total-subtotal; si hay IEPS o retenciones mezcladas, afinar leyendo
    // el nodo de Impuestos del CFDI.
    let meses: Vec<Mes> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM fecha)::int AS mes,
                COALESCE(SUM(total-subtotal) FILTER (WHERE emitido     AND tipo='I'), 0) AS trasladado,
                COALESCE(SUM(total-subtotal) FILTER (WHERE NOT emitido AND tipo='I' AND uso_cfdi <> 'S01'), 0) AS acreditable
         FROM cfdis
         WHERE usuario_id = $1 AND EXTRACT(YEAR FROM fecha) = $2
         GROUP BY mes ORDER BY mes",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;

    let trasladado: Decimal = meses.iter().map(|m| m.trasladado).sum();
    let acreditable: Decimal = meses.iter().map(|m| m.acreditable).sum();

    Ok(Pagina {
        anio,
        trasladado: pesos(trasladado),
        acreditable: pesos(acreditable),
        diferencia: pesos(trasladado - acreditable),
        meses,
    })
}
