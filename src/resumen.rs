//! Resumen anual: ingresos, gastos e IVA por mes, calculados desde los CFDI.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
use rust_decimal::Decimal;

use crate::{AppError, AppState, ahora_mx, auth::Usuario};

#[derive(sqlx::FromRow)]
pub struct Mes {
    mes: i32,
    ingresos: Decimal,
    gastos: Decimal,
    iva_trasladado: Decimal,
    iva_acreditable: Decimal,
}

const NOMBRES: [&str; 12] = [
    "Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio",
    "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre",
];

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

impl Mes {
    pub fn nombre(&self) -> &'static str {
        NOMBRES.get((self.mes - 1) as usize).copied().unwrap_or("?")
    }
    pub fn ingresos_texto(&self) -> String { pesos(self.ingresos) }
    pub fn gastos_texto(&self) -> String { pesos(self.gastos) }
    pub fn balance_texto(&self) -> String { pesos(self.ingresos - self.gastos) }
}

#[derive(Template, WebTemplate)]
#[template(path = "resumen.html")]
pub struct Pagina {
    anio: i32,
    meses: Vec<Mes>,
    // Totales del año.
    ingresos: String,
    gastos: String,
    balance: String,
    iva_trasladado: String,
    iva_acreditable: String,
    iva_por_pagar: String,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Pagina, AppError> {
    let anio = ahora_mx().year();
    // Ingresos = CFDI de ingreso emitidos por mí; gastos = de ingreso que recibí (me facturaron).
    // IVA = total - subtotal (no lo guardamos aparte). Egresos (notas de crédito) y pagos no entran aquí.
    // ponytail: ISR estimado necesita lógica por régimen; se agrega cuando el perfil tenga régimen activo.
    let meses: Vec<Mes> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM fecha)::int AS mes,
                COALESCE(SUM(total)          FILTER (WHERE emitido     AND tipo = 'I'), 0) AS ingresos,
                COALESCE(SUM(total)          FILTER (WHERE NOT emitido AND tipo = 'I'), 0) AS gastos,
                COALESCE(SUM(total-subtotal) FILTER (WHERE emitido     AND tipo = 'I'), 0) AS iva_trasladado,
                COALESCE(SUM(total-subtotal) FILTER (WHERE NOT emitido AND tipo = 'I'), 0) AS iva_acreditable
         FROM cfdis
         WHERE usuario_id = $1 AND EXTRACT(YEAR FROM fecha) = $2
         GROUP BY mes ORDER BY mes",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;

    let suma = |f: &dyn Fn(&Mes) -> Decimal| meses.iter().map(f).sum::<Decimal>();
    let ingresos = suma(&|m| m.ingresos);
    let gastos = suma(&|m| m.gastos);
    let iva_t = suma(&|m| m.iva_trasladado);
    let iva_a = suma(&|m| m.iva_acreditable);

    Ok(Pagina {
        anio,
        ingresos: pesos(ingresos),
        gastos: pesos(gastos),
        balance: pesos(ingresos - gastos),
        iva_trasladado: pesos(iva_t),
        iva_acreditable: pesos(iva_a),
        iva_por_pagar: pesos(iva_t - iva_a),
        meses,
    })
}
