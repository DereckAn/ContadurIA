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
    /// Monto deducible extra a simular (ej. aportación a PPR o gasto médico).
    simular: Option<Decimal>,
}

fn favor_cargo(resultado: Decimal) -> (String, String) {
    if resultado < Decimal::ZERO {
        (pesos(-resultado), String::new())
    } else {
        (String::new(), pesos(resultado))
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "anual.html")]
pub struct Pagina {
    anio: i32,
    sin_tabla: bool,
    ingreso: String,
    intereses: String,
    isr_intereses: String,
    hay_intereses: bool,
    deducciones: String,
    tope: String,
    tope_restante: String,
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
    // Simulador de deducciones.
    sim_mostrar: bool,
    sim_input: String,
    sim_monto: String,
    sim_a_favor: String,
    sim_a_cargo: String,
    sim_ahorro: String,
    // Proyección de cierre de año.
    proy_mostrar: bool,
    proy_meses: i64,
    proy_ingreso: String,
    proy_a_favor: String,
    proy_a_cargo: String,
}

/// Resultado del cálculo anual (números crudos, para la página y el expediente).
pub struct Calculo {
    pub anio: i32,
    pub sin_tabla: bool,
    pub ingreso: Decimal,
    /// Intereses de inversiones (GBM). Nominales: el interés real que pide la ley es menor, pero
    /// sólo lo da la constancia anual. Usar el nominal sobreestima el impuesto (estimación prudente).
    pub intereses: Decimal,
    /// ISR que retuvieron las casas de bolsa sobre esos intereses; se acredita.
    pub isr_intereses: Decimal,
    pub deducciones: Decimal,
    pub tope: Decimal,
    pub base: Decimal,
    pub isr_causado: Decimal,
    pub subsidio: Decimal,
    pub retenido: Decimal,
    pub resultado: Decimal, // negativo = saldo a favor
}

pub async fn calcular(db: &sqlx::PgPool, id: uuid::Uuid, anio: i32) -> Result<(Calculo, Vec<(String, i64, Decimal)>), AppError> {
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

    // Intereses de inversiones en pesos (las cuentas en dólares no retienen ISR mexicano).
    let (intereses, isr_intereses): (Decimal, Decimal) = sqlx::query_as(
        "SELECT COALESCE(SUM(intereses),0), COALESCE(SUM(isr_retenido),0) FROM inversiones
         WHERE usuario_id = $1 AND moneda = 'MXN' AND EXTRACT(YEAR FROM periodo) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_one(db)
    .await?;

    let tope = fiscal::tope_deducciones(ingreso + intereses);
    let deducciones = suma_deducibles.min(tope);
    let base = (ingreso + intereses - deducciones).max(Decimal::ZERO);
    let (isr_causado, sin_tabla) = match fiscal::isr_anual(anio, base) {
        Some(v) => (v, false),
        None => (Decimal::ZERO, true),
    };

    Ok((
        Calculo {
            anio,
            sin_tabla,
            ingreso,
            intereses,
            isr_intereses,
            deducciones,
            tope,
            base,
            isr_causado,
            subsidio,
            retenido,
            resultado: isr_causado - subsidio - retenido - isr_intereses,
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

    let (a_favor, a_cargo) = favor_cargo(c.resultado);
    let tope_restante = (c.tope - c.deducciones).max(Decimal::ZERO);

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

    // --- Simulador: deducible extra, limitado por el tope restante ---
    let sim = q.simular.unwrap_or(Decimal::ZERO).max(Decimal::ZERO);
    let sim_mostrar = sim > Decimal::ZERO && !c.sin_tabla;
    let (mut sim_a_favor, mut sim_a_cargo, mut sim_ahorro) = (String::new(), String::new(), String::new());
    if sim_mostrar {
        let nueva_ded = (c.deducciones + sim).min(c.tope);
        let nueva_base = (c.ingreso + c.intereses - nueva_ded).max(Decimal::ZERO);
        let nuevo_isr = fiscal::isr_anual(anio, nueva_base).unwrap_or(c.isr_causado);
        let nuevo_resultado = nuevo_isr - c.subsidio - c.retenido - c.isr_intereses;
        let (f, cg) = favor_cargo(nuevo_resultado);
        sim_a_favor = f;
        sim_a_cargo = cg;
        sim_ahorro = pesos((c.isr_causado - nuevo_isr).max(Decimal::ZERO));
    }

    // --- Proyección de cierre de año (solo el año en curso, con datos parciales) ---
    let meses: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT EXTRACT(MONTH FROM fecha))::bigint
         FROM cfdis WHERE usuario_id = $1 AND tipo = 'N' AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_one(&st.db)
    .await?;
    let proy_mostrar = anio == crate::ahora_mx().date().year() && (1..12).contains(&meses) && !c.sin_tabla;
    let (mut proy_ingreso, mut proy_a_favor, mut proy_a_cargo) = (String::new(), String::new(), String::new());
    if proy_mostrar {
        let factor = Decimal::from(12) / Decimal::from(meses);
        let ingreso_a = (c.ingreso * factor).round_dp(2);
        let base_a = (ingreso_a + c.intereses - c.deducciones).max(Decimal::ZERO);
        let isr_a = fiscal::isr_anual(anio, base_a).unwrap_or(Decimal::ZERO);
        let resultado_a = isr_a - c.subsidio * factor - c.retenido * factor - c.isr_intereses;
        let (f, cg) = favor_cargo(resultado_a);
        proy_ingreso = pesos(ingreso_a);
        proy_a_favor = f;
        proy_a_cargo = cg;
    }

    Ok(Pagina {
        anio,
        sin_tabla: c.sin_tabla,
        ingreso: pesos(c.ingreso),
        intereses: pesos(c.intereses),
        isr_intereses: pesos(c.isr_intereses),
        hay_intereses: !c.intereses.is_zero() || !c.isr_intereses.is_zero(),
        deducciones: pesos(c.deducciones),
        tope: pesos(c.tope),
        tope_restante: pesos(tope_restante),
        base: pesos(c.base),
        isr_causado: pesos(c.isr_causado),
        subsidio: pesos(c.subsidio),
        retenido: pesos(c.retenido),
        a_favor,
        a_cargo,
        deducibles,
        gastos_sin_factura: pesos(gastos_sin_factura),
        n_sin_factura,
        sim_mostrar,
        sim_input: if sim > Decimal::ZERO { sim.normalize().to_string() } else { String::new() },
        sim_monto: pesos(sim),
        sim_a_favor,
        sim_a_cargo,
        sim_ahorro,
        proy_mostrar,
        proy_meses: meses,
        proy_ingreso,
        proy_a_favor,
        proy_a_cargo,
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
         Intereses de inversiones:       {int}   (nominales; el real de la constancia anual es menor)\n\
         Deducciones personales:         {ded}   (tope de ley: {tope})\n\
         Base gravable:                  {base}\n\
         ISR causado (tarifa anual):     {isr}\n\
         Subsidio al empleo:             {sub}\n\
         ISR retenido por el patrón:     {ret}\n\
         ISR retenido por inversiones:   {isr_int}\n\
         -----------------------------------------\n\
         {resultado}\n\n\
         Tarifa anual 2026: Anexo 8 RMF 2026 (DOF 28-dic-2025). UMA 2026: INEGI.\n",
        anio = c.anio, ing = pesos(c.ingreso), ded = pesos(c.deducciones), tope = pesos(c.tope),
        base = pesos(c.base), isr = pesos(c.isr_causado), sub = pesos(c.subsidio), ret = pesos(c.retenido),
        int = pesos(c.intereses), isr_int = pesos(c.isr_intereses),
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
