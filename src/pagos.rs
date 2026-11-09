//! Pagos provisionales mensuales de ISR, según el régimen del usuario.
//! RESICO (626): tasa fija sobre lo cobrado. 612/606/625: tarifa acumulada menos lo ya pagado.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Query, State};
use rust_decimal::Decimal;
use serde::Deserialize;
use time::{Date, Month, macros::format_description};

use crate::{AppError, AppState, auth::Usuario, fiscal};

const MESES: [&str; 12] = [
    "Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio",
    "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre",
];

/// Regímenes que presentan pagos provisionales, en orden de preferencia si hay varios.
const PROVISIONALES: [(&str, &str); 4] = [
    ("626", "RESICO"),
    ("612", "Actividad empresarial y profesional"),
    ("606", "Arrendamiento"),
    ("625", "Plataformas tecnológicas"),
];

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

/// Día 17 del mes siguiente (fecha límite del pago provisional).
fn limite(anio: i32, mes: u32) -> String {
    let (a, m) = if mes == 12 { (anio + 1, 1u32) } else { (anio, mes + 1) };
    Date::from_calendar_date(a, Month::try_from(m as u8).unwrap(), 17)
        .map(|d| d.format(format_description!("[day]/[month]/[year]")).unwrap_or_default())
        .unwrap_or_default()
}

pub struct Fila {
    pub mes: String,
    pub ingresos: String,
    pub deducciones: String,
    pub base: String,
    pub pago: String,
    pub limite: String,
}

#[derive(Deserialize)]
pub struct Params {
    anio: Option<i32>,
}

#[derive(Template, WebTemplate)]
#[template(path = "pagos.html")]
pub struct Pagina {
    anio: i32,
    regimen: Option<String>,
    es_resico: bool,
    sin_tabla: bool,
    filas: Vec<Fila>,
    total: String,
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());

    let regimenes: Vec<String> = sqlx::query_scalar("SELECT regimenes FROM perfiles WHERE usuario_id = $1")
        .bind(id)
        .fetch_optional(&st.db)
        .await?
        .unwrap_or_default();
    let elegido = PROVISIONALES.iter().find(|(c, _)| regimenes.iter().any(|r| r == c));

    let Some((codigo, nombre)) = elegido else {
        return Ok(Pagina {
            anio,
            regimen: None,
            es_resico: false,
            sin_tabla: false,
            filas: vec![],
            total: pesos(Decimal::ZERO),
        });
    };
    let es_resico = *codigo == "626";

    // Ingresos y deducciones facturados por mes.
    let por_mes: Vec<(i32, Decimal, Decimal)> = sqlx::query_as(
        "SELECT EXTRACT(MONTH FROM fecha)::int AS mes,
                COALESCE(SUM(total) FILTER (WHERE emitido AND tipo='I'),0) AS ingresos,
                COALESCE(SUM(total) FILTER (WHERE NOT emitido AND tipo='I' AND uso_cfdi<>'S01'),0) AS deducciones
         FROM cfdis WHERE usuario_id = $1 AND EXTRACT(YEAR FROM fecha) = $2
         GROUP BY mes ORDER BY mes",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;

    let mut ingreso_mes = [Decimal::ZERO; 13];
    let mut deducc_mes = [Decimal::ZERO; 13];
    for (m, i, d) in &por_mes {
        if (1..=12).contains(m) {
            ingreso_mes[*m as usize] = *i;
            deducc_mes[*m as usize] = *d;
        }
    }

    let mut filas = Vec::new();
    let mut total = Decimal::ZERO;
    let mut sin_tabla = false;
    let (mut ing_acum, mut ded_acum, mut isr_prev) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);

    for m in 1u32..=12 {
        let ing = ingreso_mes[m as usize];
        let ded = deducc_mes[m as usize];
        ing_acum += ing;
        ded_acum += ded;

        let (pago, ingresos_col, deducc_col, base_col) = if es_resico {
            // RESICO: tasa fija sobre lo cobrado del mes, sin deducciones.
            (fiscal::isr_resico(ing), ing, Decimal::ZERO, ing)
        } else {
            let base = (ing_acum - ded_acum).max(Decimal::ZERO);
            match fiscal::isr_acumulado(anio, base, m) {
                Some(isr) => {
                    let pago = (isr - isr_prev).max(Decimal::ZERO);
                    isr_prev = isr;
                    (pago, ing_acum, ded_acum, base)
                }
                None => {
                    sin_tabla = true;
                    (Decimal::ZERO, ing_acum, ded_acum, base)
                }
            }
        };

        // Solo muestra meses con actividad.
        if ing > Decimal::ZERO || pago > Decimal::ZERO {
            total += pago;
            filas.push(Fila {
                mes: MESES[(m - 1) as usize].to_string(),
                ingresos: pesos(ingresos_col),
                deducciones: if es_resico { "—".into() } else { pesos(deducc_col) },
                base: pesos(base_col),
                pago: pesos(pago),
                limite: limite(anio, m),
            });
        }
    }

    Ok(Pagina {
        anio,
        regimen: Some(format!("{codigo} · {nombre}")),
        es_resico,
        sin_tabla,
        filas,
        total: pesos(total),
    })
}
