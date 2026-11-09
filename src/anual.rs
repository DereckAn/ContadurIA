//! Declaración anual (personas físicas, sueldos): ISR del ejercicio a partir de la nómina.

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::{Query, State}, response::IntoResponse};
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

/// Resultado del cálculo anual (números crudos, para la página y el expediente).
pub struct Calculo {
    pub anio: i32,
    pub sin_tabla: bool,
    pub ingreso: Decimal,
    pub deducciones: Decimal,
    pub tope: Decimal,
    pub base: Decimal,
    pub isr_causado: Decimal,
    pub subsidio: Decimal,
    pub retenido: Decimal,
    pub resultado: Decimal, // negativo = saldo a favor
}

async fn calcular(db: &sqlx::PgPool, id: uuid::Uuid, anio: i32) -> Result<(Calculo, Vec<(String, i64, Decimal)>), AppError> {
    // Nómina del ejercicio.
    let xmls: Vec<(String,)> = sqlx::query_as(
        "SELECT xml FROM cfdis WHERE usuario_id = $1 AND tipo = 'N' AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(db)
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
    .fetch_all(db)
    .await?;
    let suma_deducibles: Decimal = filas.iter().map(|(_, _, m)| *m).sum();

    let tope = fiscal::tope_deducciones(ingreso);
    let deducciones = suma_deducibles.min(tope);
    let base = (ingreso - deducciones).max(Decimal::ZERO);
    let (isr_causado, sin_tabla) = match fiscal::isr_anual(anio, base) {
        Some(v) => (v, false),
        None => (Decimal::ZERO, true),
    };

    Ok((
        Calculo {
            anio,
            sin_tabla,
            ingreso,
            deducciones,
            tope,
            base,
            isr_causado,
            subsidio,
            retenido,
            resultado: isr_causado - subsidio - retenido,
        },
        filas,
    ))
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());
    let (c, filas) = calcular(&st.db, id, anio).await?;

    let deducibles: Vec<FilaDeducible> = filas
        .iter()
        .map(|(uso, n, m)| FilaDeducible {
            concepto: DEDUCIBLES.iter().find(|(cod, _)| cod == uso).map(|(_, d)| *d).unwrap_or(uso).to_string(),
            cuenta: *n,
            monto: pesos(*m),
        })
        .collect();

    let (a_favor, a_cargo) = if c.resultado < Decimal::ZERO {
        (pesos(-c.resultado), String::new())
    } else {
        (String::new(), pesos(c.resultado))
    };

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
        sin_tabla: c.sin_tabla,
        ingreso: pesos(c.ingreso),
        deducciones: pesos(c.deducciones),
        tope: pesos(c.tope),
        base: pesos(c.base),
        isr_causado: pesos(c.isr_causado),
        subsidio: pesos(c.subsidio),
        retenido: pesos(c.retenido),
        a_favor,
        a_cargo,
        deducibles,
        gastos_sin_factura: pesos(gastos_sin_factura),
        n_sin_factura,
    })
}

// --- Expediente anual exportable (ZIP con resumen, CSV y XMLs) ---

fn resumen_texto(c: &Calculo) -> String {
    let resultado = if c.resultado < Decimal::ZERO {
        format!("Saldo a FAVOR: {}", pesos(-c.resultado))
    } else {
        format!("ISR a CARGO: {}", pesos(c.resultado))
    };
    format!(
        "EXPEDIENTE DE DECLARACIÓN ANUAL {anio}\n\
         (Estimación de ContadurIA. No es un cálculo oficial; verifica en el SAT.)\n\n\
         Ingresos por sueldos (gravado): {ing}\n\
         Deducciones personales:         {ded}   (tope de ley: {tope})\n\
         Base gravable:                  {base}\n\
         ISR causado (tarifa anual):     {isr}\n\
         Subsidio al empleo:             {sub}\n\
         ISR retenido por el patrón:     {ret}\n\
         -----------------------------------------\n\
         {resultado}\n\n\
         Tarifa anual 2026: Anexo 8 RMF 2026 (DOF 28-dic-2025). UMA 2026: INEGI.\n",
        anio = c.anio, ing = pesos(c.ingreso), ded = pesos(c.deducciones), tope = pesos(c.tope),
        base = pesos(c.base), isr = pesos(c.isr_causado), sub = pesos(c.subsidio), ret = pesos(c.retenido),
    )
}

fn csv_campo(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub async fn expediente(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<axum::response::Response, AppError> {
    use std::io::{Cursor, Write};

    let anio = q.anio.unwrap_or_else(|| crate::ahora_mx().date().year());
    let (c, _) = calcular(&st.db, id, anio).await?;

    #[derive(sqlx::FromRow)]
    struct Row {
        uuid: String,
        fecha: time::PrimitiveDateTime,
        tipo: String,
        emitido: bool,
        emisor_rfc: String,
        emisor_nombre: String,
        receptor_rfc: String,
        receptor_nombre: String,
        uso_cfdi: String,
        total: Decimal,
        xml: String,
    }
    let cfdis: Vec<Row> = sqlx::query_as(
        "SELECT uuid, fecha, tipo, emitido, emisor_rfc, emisor_nombre, receptor_rfc, receptor_nombre, uso_cfdi, total, xml
         FROM cfdis WHERE usuario_id = $1 AND EXTRACT(YEAR FROM fecha) = $2 ORDER BY fecha",
    )
    .bind(id)
    .bind(anio)
    .fetch_all(&st.db)
    .await?;

    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opt = zip::write::SimpleFileOptions::default();

        zip.start_file("resumen.txt", opt)?;
        zip.write_all(resumen_texto(&c).as_bytes())?;

        zip.start_file("cfdis.csv", opt)?;
        zip.write_all(b"fecha,tipo,emitido,emisor_rfc,emisor_nombre,receptor_rfc,receptor_nombre,uso,total\n")?;
        let fmt = time::macros::format_description!("[year]-[month]-[day]");
        for r in &cfdis {
            let linea = format!(
                "{},{},{},{},{},{},{},{},{}\n",
                r.fecha.date().format(fmt).unwrap_or_default(),
                r.tipo,
                if r.emitido { "emitido" } else { "recibido" },
                csv_campo(&r.emisor_rfc),
                csv_campo(&r.emisor_nombre),
                csv_campo(&r.receptor_rfc),
                csv_campo(&r.receptor_nombre),
                r.uso_cfdi,
                r.total,
            );
            zip.write_all(linea.as_bytes())?;
        }

        for r in &cfdis {
            zip.start_file(format!("xml/{}.xml", r.uuid), opt)?;
            zip.write_all(r.xml.as_bytes())?;
        }
        zip.finish()?;
    }

    use axum::http::header;
    let encabezados = [
        (header::CONTENT_TYPE, "application/zip".to_string()),
        (header::CONTENT_DISPOSITION, format!("attachment; filename=\"expediente-{anio}.zip\"")),
    ];
    Ok((encabezados, buf).into_response())
}
