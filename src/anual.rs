//! Declaración anual (personas físicas, sueldos): ISR del ejercicio a partir de la nómina.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Query, State};
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{AppError, AppState, auth::Usuario, fiscal};

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

/// Usos de CFDI que son deducciones personales (art. 151 LISR).
const DEDUCIBLES: [(&str, &str); 10] = [
    ("D01", "Honorarios médicos, dentales y hospitalarios"),
    ("D02", "Gastos médicos por incapacidad o discapacidad"),
    ("D03", "Gastos funerales"),
    ("D04", "Donativos"),
    ("D05", "Intereses reales de crédito hipotecario"),
    ("D06", "Aportaciones voluntarias al SAR"),
    ("D07", "Primas de seguros de gastos médicos"),
    ("D08", "Transporte escolar obligatorio"),
    ("D09", "Depósitos en cuentas de ahorro / PPR"),
    ("D10", "Colegiaturas"),
];

/// Extrae del XML de un CFDI de nómina: (gravado, ISR retenido, subsidio causado).
fn nomina(xml: &str) -> (Decimal, Decimal, Decimal) {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    };
    let num = |nodo: Option<roxmltree::Node>, attr: &str| {
        nodo.and_then(|n| n.attribute(attr)).and_then(|v| v.parse::<Decimal>().ok()).unwrap_or(Decimal::ZERO)
    };
    let buscar = |tag: &str| doc.descendants().find(|n| n.has_tag_name(tag));
    (
        num(buscar("Percepciones"), "TotalGravado"),
        num(buscar("Deducciones"), "TotalImpuestosRetenidos"),
        num(buscar("SubsidioAlEmpleo"), "SubsidioCausado"),
    )
}

pub struct FilaDeducible {
    pub concepto: String,
    pub cuenta: i64,
    pub monto: String,
}

#[derive(Deserialize)]
pub struct Params {
    anio: Option<i32>,
}

#[derive(Template, WebTemplate)]
#[template(path = "anual.html")]
pub struct Pagina {
    anio: i32,
    sin_tabla: bool,
    ingreso: String,
    deducciones: String,
    tope: String,
    base: String,
    isr_causado: String,
    subsidio: String,
    retenido: String,
    a_favor: String,
    a_cargo: String,
    deducibles: Vec<FilaDeducible>,
    // Pistas para pedir facturas.
    gastos_sin_factura: String,
    n_sin_factura: i64,
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());

    // Nómina del ejercicio.
    let xmls: Vec<(String,)> = sqlx::query_as(
        "SELECT xml FROM cfdis WHERE usuario_id = $1 AND tipo = 'N' AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;
    let (mut ingreso, mut retenido, mut subsidio) = (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    for (xml,) in &xmls {
        let (g, r, s) = nomina(xml);
        ingreso += g;
        retenido += r;
        subsidio += s;
    }

    // Deducciones personales por uso de CFDI.
    let filas: Vec<(String, i64, Decimal)> = sqlx::query_as(
        "SELECT uso_cfdi, count(*), COALESCE(SUM(total),0)
         FROM cfdis
         WHERE usuario_id = $1 AND NOT emitido AND EXTRACT(YEAR FROM fecha) = $2
           AND uso_cfdi = ANY($3)
         GROUP BY uso_cfdi",
    )
    .bind(id)
    .bind(anio)
    .bind(DEDUCIBLES.iter().map(|(c, _)| *c).collect::<Vec<_>>())
    .fetch_all(&st.db)
    .await?;
    let suma_deducibles: Decimal = filas.iter().map(|(_, _, m)| *m).sum();
    let deducibles: Vec<FilaDeducible> = filas
        .iter()
        .map(|(uso, n, m)| FilaDeducible {
            concepto: DEDUCIBLES.iter().find(|(c, _)| c == uso).map(|(_, d)| *d).unwrap_or(uso).to_string(),
            cuenta: *n,
            monto: pesos(*m),
        })
        .collect();

    let tope = fiscal::tope_deducciones(ingreso);
    let deducciones = suma_deducibles.min(tope);
    let base = (ingreso - deducciones).max(Decimal::ZERO);

    let (isr_causado, sin_tabla) = match fiscal::isr_anual(anio, base) {
        Some(v) => (v, false),
        None => (Decimal::ZERO, true),
    };
    let resultado = isr_causado - subsidio - retenido; // negativo = a favor
    let (a_favor, a_cargo) = if resultado < Decimal::ZERO {
        (pesos(-resultado), String::new())
    } else {
        (String::new(), pesos(resultado))
    };

    // Gastos del banco sin factura (pistas para pedir CFDI).
    let (n_sin_factura, gastos_sin_factura): (i64, Decimal) = sqlx::query_as(
        "SELECT count(*), COALESCE(SUM(cargo),0)
         FROM movimientos
         WHERE usuario_id = $1 AND cargo IS NOT NULL AND cfdi_uuid IS NULL
           AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_one(&st.db)
    .await?;

    Ok(Pagina {
        anio,
        sin_tabla,
        ingreso: pesos(ingreso),
        deducciones: pesos(deducciones),
        tope: pesos(tope),
        base: pesos(base),
        isr_causado: pesos(isr_causado),
        subsidio: pesos(subsidio),
        retenido: pesos(retenido),
        a_favor,
        a_cargo,
        deducibles,
        gastos_sin_factura: pesos(gastos_sin_factura),
        n_sin_factura,
    })
}
