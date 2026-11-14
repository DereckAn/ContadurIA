//! Informe mensual: resumen de un mes (ingresos, gastos, deducibles, IVA y obligaciones).

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::{Query, State}, response::{IntoResponse, Redirect, Response}};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor, message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;
use time::{Date, Month};
use uuid::Uuid;

use crate::{AppError, AppState, auth::Usuario, calendario, var};

const MESES: [&str; 12] = [
    "Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio",
    "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre",
];

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

#[derive(Deserialize)]
pub struct Params {
    mes: Option<String>, // "AAAA-MM"
    enviado: Option<String>,
}

/// Primer día del mes a reportar: el del parámetro, o el mes pasado por defecto.
fn mes_objetivo(param: &Option<String>, hoy: Date) -> (i32, Month) {
    if let Some(s) = param
        && let Some((a, m)) = s.split_once('-')
        && let (Ok(a), Ok(m)) = (a.parse::<i32>(), m.parse::<u8>())
        && let Ok(m) = Month::try_from(m)
    {
        return (a, m);
    }
    // Mes anterior.
    if hoy.month() == Month::January {
        (hoy.year() - 1, Month::December)
    } else {
        (hoy.year(), hoy.month().previous())
    }
}

#[derive(sqlx::FromRow)]
struct Banco {
    ingresos: Decimal,
    gastos: Decimal,
    gastos_facturados: Decimal,
    n_gastos: i64,
    n_gastos_fact: i64,
}

/// Datos de un informe. Público para que el envío por correo lo reutilice.
pub struct Datos {
    pub nombre: String,
    pub correo: String,
    pub mes_texto: String,
    ingresos: String,
    gastos: String,
    balance: String,
    gastos_facturados: String,
    gastos_sin_factura: String,
    n_gastos: i64,
    n_gastos_fact: i64,
    iva_acreditable: String,
    obligaciones: Vec<calendario::Obligacion>,
}

pub async fn reunir(db: &PgPool, id: Uuid, param: &Option<String>, hoy: Date) -> Result<Datos, AppError> {
    let (anio, mes) = mes_objetivo(param, hoy);
    let inicio = Date::from_calendar_date(anio, mes, 1)?;
    let fin = if mes == Month::December {
        Date::from_calendar_date(anio + 1, Month::January, 1)?
    } else {
        Date::from_calendar_date(anio, mes.next(), 1)?
    };

    let (nombre, correo, regimenes, efirma_vence): (String, String, Vec<String>, Option<Date>) = sqlx::query_as(
        "SELECT p.nombre, u.correo, p.regimenes, p.efirma_vence
         FROM perfiles p JOIN usuarios u ON u.id = p.usuario_id WHERE p.usuario_id = $1",
    )
    .bind(id)
    .fetch_one(db)
    .await?;

    let b: Banco = sqlx::query_as(
        "SELECT COALESCE(SUM(abono),0) AS ingresos,
                COALESCE(SUM(cargo),0) AS gastos,
                COALESCE(SUM(cargo) FILTER (WHERE cfdi_uuid IS NOT NULL),0) AS gastos_facturados,
                count(*) FILTER (WHERE cargo IS NOT NULL) AS n_gastos,
                count(*) FILTER (WHERE cargo IS NOT NULL AND cfdi_uuid IS NOT NULL) AS n_gastos_fact
         FROM movimientos WHERE usuario_id = $1 AND fecha >= $2 AND fecha < $3",
    )
    .bind(id)
    .bind(inicio)
    .bind(fin)
    .fetch_one(db)
    .await?;

    let iva: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(total - subtotal) FILTER (WHERE NOT emitido AND tipo = 'I'),0)
         FROM cfdis WHERE usuario_id = $1 AND fecha >= $2 AND fecha < $3",
    )
    .bind(id)
    .bind(inicio)
    .bind(fin)
    .fetch_one(db)
    .await?;

    let obligaciones = calendario::obligaciones(&regimenes, efirma_vence, hoy, 45);

    Ok(Datos {
        nombre,
        correo,
        mes_texto: format!("{} {}", MESES[mes as usize - 1], anio),
        ingresos: pesos(b.ingresos),
        gastos: pesos(b.gastos),
        balance: pesos(b.ingresos - b.gastos),
        gastos_facturados: pesos(b.gastos_facturados),
        gastos_sin_factura: pesos(b.gastos - b.gastos_facturados),
        n_gastos: b.n_gastos,
        n_gastos_fact: b.n_gastos_fact,
        iva_acreditable: pesos(iva),
        obligaciones,
    })
}

#[derive(Template, WebTemplate)]
#[template(path = "informe.html")]
pub struct Pagina {
    d: Datos,
    enviado: bool,
}

pub async fn pagina(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Pagina, AppError> {
    let d = reunir(&st.db, id, &q.mes, crate::ahora_mx().date()).await?;
    Ok(Pagina { d, enviado: q.enviado.is_some() })
}

// --- Envío por correo ---

/// Cuerpo HTML sencillo del correo (sin menú ni estilos de la app).
fn cuerpo(d: &Datos) -> String {
    let mut o = String::new();
    for ob in &d.obligaciones {
        o.push_str(&format!("<li>{} — {} ({})</li>", ob.fecha_texto(), ob.titulo, ob.faltan_texto()));
    }
    format!(
        "<h2>Informe de {mes}</h2><p>Hola, {nombre}.</p>\
         <ul><li>Ingresos: <b>{ing}</b></li><li>Gastos: <b>{gas}</b></li><li>Balance: <b>{bal}</b></li></ul>\
         <p>Gastos con factura: <b>{fact}</b> ({nf} de {ng}). Sin factura: {sf}. IVA acreditable: {iva}.</p>\
         <h3>Próximas obligaciones</h3><ul>{o}</ul>\
         <p style=\"color:#888;font-size:12px\">Generado por ContadurIA. No es un cálculo oficial de impuestos.</p>",
        mes = d.mes_texto, nombre = d.nombre, ing = d.ingresos, gas = d.gastos, bal = d.balance,
        fact = d.gastos_facturados, nf = d.n_gastos_fact, ng = d.n_gastos, sf = d.gastos_sin_factura,
        iva = d.iva_acreditable, o = o,
    )
}

/// ¿Hay SMTP configurado para mandar correos?
pub fn hay_correo() -> bool {
    !var("SMTP_HOST", "").is_empty()
}

/// Manda un correo HTML con el SMTP de las variables de entorno.
pub async fn enviar_correo(para: &str, asunto: &str, html: String) -> Result<(), AppError> {
    let host = var("SMTP_HOST", "");
    if host.is_empty() {
        return Err(AppError::usuario("Falta configurar el correo (variables SMTP_HOST, SMTP_USER, SMTP_PASS)."));
    }
    let usuario = var("SMTP_USER", "");
    let clave = var("SMTP_PASS", "").replace(' ', ""); // Gmail muestra la clave con espacios
    let remitente = var("SMTP_FROM", &usuario);
    let puerto: u16 = var("SMTP_PORT", "587").parse().unwrap_or(587);

    let correo = lettre::Message::builder()
        .from(remitente.parse().map_err(|_| AppError::usuario("SMTP_FROM inválido."))?)
        .to(para.parse().map_err(|_| AppError::usuario("Tu correo no es válido."))?)
        .subject(asunto)
        .header(ContentType::TEXT_HTML)
        .body(html)?;

    let transporte = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)?
        .port(puerto)
        .credentials(Credentials::new(usuario, clave))
        .build();
    transporte.send(correo).await?;
    Ok(())
}

pub async fn enviar(db: &PgPool, id: Uuid, param: &Option<String>, hoy: Date) -> Result<(), AppError> {
    let d = reunir(db, id, param, hoy).await?;
    enviar_correo(&d.correo, &format!("Informe de {}", d.mes_texto), cuerpo(&d)).await
}

pub async fn enviar_handler(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    Query(q): Query<Params>,
) -> Result<Response, AppError> {
    enviar(&st.db, id, &q.mes, crate::ahora_mx().date()).await?;
    Ok(Redirect::to("/informe?enviado=1").into_response())
}
