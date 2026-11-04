//! Descarga Masiva de CFDI del SAT. El navegador firma con la e.firma (WebAssembly)
//! y este módulo sólo reenvía al SAT los mensajes ya firmados: la llave privada
//! nunca llega al servidor. El token del SAT (dura 5 min) vive en la sesión.

use askama::Template;
use askama_web::WebTemplate;
use axum::{Json, extract::State, http::StatusCode};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, macros::format_description};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{AppError, AppState, ahora_mx, auth::Usuario, cfdi};

const AUTENTICACION: &str = "https://cfdidescargamasivasolicitud.clouda.sat.gob.mx/Autenticacion/Autenticacion.svc";
const SOLICITUD: &str = "https://cfdidescargamasivasolicitud.clouda.sat.gob.mx/SolicitaDescargaService.svc";
const VERIFICACION: &str = "https://cfdidescargamasivasolicitud.clouda.sat.gob.mx/VerificaSolicitudDescargaService.svc";
const DESCARGA: &str = "https://cfdidescargamasiva.clouda.sat.gob.mx/DescargaMasivaService.svc";
const ACCION: &str = "http://DescargaMasivaTerceros.sat.gob.mx";

const TOKEN: &str = "sat_token";
const FORMATO_SAT: &[time::format_description::BorrowedFormatItem] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");

// ---------- Páginas ----------

#[derive(sqlx::FromRow)]
pub struct Solicitud {
    tipo: String,
    fecha_inicial: PrimitiveDateTime,
    fecha_final: PrimitiveDateTime,
    estado: String,
    mensaje: String,
}

impl Solicitud {
    pub fn periodo(&self) -> String {
        let f = format_description!("[day]/[month]/[year]");
        format!("{} al {}", self.fecha_inicial.format(f).unwrap_or_default(), self.fecha_final.format(f).unwrap_or_default())
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "sat.html")]
pub struct PaginaSat {
    rfc: String,
    solicitudes: Vec<Solicitud>,
    total_cfdi: i64,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<PaginaSat, AppError> {
    let rfc = rfc_perfil(&st.db, id).await?;
    let solicitudes = sqlx::query_as(
        "SELECT tipo, fecha_inicial, fecha_final, estado, mensaje FROM sat_solicitudes
         WHERE usuario_id = $1 ORDER BY creada DESC LIMIT 20",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let total_cfdi = sqlx::query_scalar("SELECT count(*) FROM cfdis WHERE usuario_id = $1").bind(id).fetch_one(&st.db).await?;
    Ok(PaginaSat { rfc, solicitudes, total_cfdi })
}

async fn rfc_perfil(db: &PgPool, id: Uuid) -> Result<String, AppError> {
    sqlx::query_scalar("SELECT rfc FROM perfiles WHERE usuario_id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError::usuario("Primero llena tu perfil fiscal."))
}

// ---------- Qué debe firmar el navegador ----------

#[derive(Serialize)]
#[serde(tag = "accion", rename_all = "lowercase")]
pub enum Tarea {
    Solicitar { tipo: &'static str, inicio: String, fin: String },
    Verificar { id: String },
    Descargar { id: String },
}

pub async fn pendientes(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Json<Vec<Tarea>>, AppError> {
    let mut tareas = Vec::new();
    let ahora = ahora_mx();

    for tipo in ["emitidos", "recibidos"] {
        let ultima: Option<(String, OffsetDateTime)> = sqlx::query_as(
            "SELECT estado, creada FROM sat_solicitudes WHERE usuario_id = $1 AND tipo = $2 ORDER BY creada DESC LIMIT 1",
        )
        .bind(id)
        .bind(tipo)
        .fetch_optional(&st.db)
        .await?;
        if let Some((estado, creada)) = ultima {
            // Una a la vez por tipo, y como máximo cada 6 h: el SAT limita cuántas solicitudes aceptas.
            if estado == "solicitada" || estado == "lista" || creada > OffsetDateTime::now_utc() - Duration::hours(6) {
                continue;
            }
        }
        let desde: Option<PrimitiveDateTime> = sqlx::query_scalar(
            "SELECT max(fecha_final) FROM sat_solicitudes WHERE usuario_id = $1 AND tipo = $2 AND estado = 'descargada'",
        )
        .bind(id)
        .bind(tipo)
        .fetch_one(&st.db)
        .await?;
        // Primera vez: desde el 1 de enero del año pasado, para cubrir la anual pendiente.
        let inicio = desde.unwrap_or_else(|| {
            PrimitiveDateTime::new(time::Date::from_calendar_date(ahora.year() - 1, Month::January, 1).unwrap(), Time::MIDNIGHT)
        });
        if ahora - inicio < Duration::minutes(1) {
            continue;
        }
        tareas.push(Tarea::Solicitar { tipo, inicio: sat(inicio), fin: sat(ahora) });
    }

    let por_verificar: Vec<String> = sqlx::query_scalar(
        "SELECT id_solicitud FROM sat_solicitudes WHERE usuario_id = $1 AND estado = 'solicitada'
         AND (verificada IS NULL OR verificada < now() - interval '1 minute')",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    tareas.extend(por_verificar.into_iter().map(|id| Tarea::Verificar { id }));

    let por_descargar: Vec<String> = sqlx::query_scalar(
        "SELECT p FROM sat_solicitudes, unnest(paquetes) p WHERE usuario_id = $1 AND estado = 'lista' AND NOT p = ANY(descargados)",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    tareas.extend(por_descargar.into_iter().map(|id| Tarea::Descargar { id }));

    Ok(Json(tareas))
}

fn sat(fecha: PrimitiveDateTime) -> String {
    fecha.format(FORMATO_SAT).unwrap_or_default()
}

// ---------- Reenvío al SAT ----------

#[derive(Deserialize)]
pub struct Envio {
    xml: String,
}

#[derive(Serialize)]
pub struct Resultado {
    mensaje: String,
}

fn ok(mensaje: impl Into<String>) -> Result<Json<Resultado>, AppError> {
    Ok(Json(Resultado { mensaje: mensaje.into() }))
}

pub async fn enviar(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    sesion: Session,
    Json(envio): Json<Envio>,
) -> Result<Json<Resultado>, AppError> {
    let doc = Document::parse(&envio.xml).map_err(|_| AppError::usuario("XML inválido."))?;
    let operacion = elemento(&doc, "Body")
        .and_then(|b| b.first_element_child())
        .ok_or_else(|| AppError::usuario("Mensaje SOAP sin cuerpo."))?;
    let nombre = operacion.tag_name().name();

    if nombre == "Autentica" {
        let respuesta = llamar(&st.http, AUTENTICACION, "http://DescargaMasivaTerceros.gob.mx/IAutenticacion/Autentica", &envio.xml, None).await?;
        let r = Document::parse(&respuesta).map_err(|_| sat_raro())?;
        let token = elemento(&r, "AutenticaResult").and_then(|n| n.text()).ok_or_else(sat_raro)?;
        // El token dura 5 minutos; se da por vencido un poco antes.
        let vence = (OffsetDateTime::now_utc() + Duration::minutes(4)).unix_timestamp();
        sesion.insert(TOKEN, (token.to_string(), vence)).await?;
        return ok("Conectado con el SAT.");
    }

    // Toda operación va firmada por el RFC solicitante: debe ser el del perfil.
    let rfc = rfc_perfil(&st.db, id).await?;
    let datos = operacion.first_element_child().ok_or_else(|| AppError::usuario("Mensaje incompleto."))?;
    if datos.attribute("RfcSolicitante") != Some(rfc.as_str()) {
        return Err(AppError::usuario("La e.firma no corresponde al RFC de tu perfil."));
    }
    let token = match sesion.get::<(String, i64)>(TOKEN).await? {
        Some((t, vence)) if vence > OffsetDateTime::now_utc().unix_timestamp() => t,
        _ => return Err(AppError(StatusCode::UNAUTHORIZED, "La conexión con el SAT venció.".into())),
    };

    match nombre {
        "SolicitaDescargaEmitidos" | "SolicitaDescargaRecibidos" => {
            let respuesta = llamar(&st.http, SOLICITUD, &format!("{ACCION}/ISolicitaDescargaService/{nombre}"), &envio.xml, Some(&token)).await?;
            let r = Document::parse(&respuesta).map_err(|_| sat_raro())?;
            let res = elemento(&r, &format!("{nombre}Result")).ok_or_else(sat_raro)?;
            let mensaje = atributo(res, "Mensaje");
            if atributo(res, "CodEstatus") != "5000" {
                return Err(AppError::usuario(&format!("El SAT rechazó la solicitud: {mensaje}")));
            }
            let fecha = |n: &str| PrimitiveDateTime::parse(datos.attribute(n).unwrap_or_default(), FORMATO_SAT);
            let (Ok(inicio), Ok(fin)) = (fecha("FechaInicial"), fecha("FechaFinal")) else {
                return Err(AppError::usuario("Fechas inválidas en la solicitud."));
            };
            let tipo = if nombre.ends_with("Emitidos") { "emitidos" } else { "recibidos" };
            sqlx::query(
                "INSERT INTO sat_solicitudes (id_solicitud, usuario_id, tipo, fecha_inicial, fecha_final, mensaje)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(atributo(res, "IdSolicitud"))
            .bind(id)
            .bind(tipo)
            .bind(inicio)
            .bind(fin)
            .bind(&mensaje)
            .execute(&st.db)
            .await?;
            ok(format!("Solicitud de {tipo} aceptada por el SAT; puede tardar de minutos a horas."))
        }

        "VerificaSolicitudDescarga" => {
            let id_solicitud = datos.attribute("IdSolicitud").unwrap_or_default();
            let respuesta = llamar(&st.http, VERIFICACION, &format!("{ACCION}/IVerificaSolicitudDescargaService/VerificaSolicitudDescarga"), &envio.xml, Some(&token)).await?;
            let r = Document::parse(&respuesta).map_err(|_| sat_raro())?;
            let res = elemento(&r, "VerificaSolicitudDescargaResult").ok_or_else(sat_raro)?;
            let paquetes: Vec<String> = res
                .children()
                .filter(|n| n.tag_name().name().eq_ignore_ascii_case("IdsPaquetes"))
                .filter_map(|n| n.text().map(str::to_string))
                .collect();
            let (estado, mensaje) = verificacion(
                &atributo(res, "CodEstatus"),
                &atributo(res, "EstadoSolicitud"),
                &atributo(res, "CodigoEstadoSolicitud"),
                &atributo(res, "Mensaje"),
                paquetes.len(),
            );
            let filas = sqlx::query(
                "UPDATE sat_solicitudes SET estado = $1, mensaje = $2, paquetes = $3, verificada = now()
                 WHERE id_solicitud = $4 AND usuario_id = $5",
            )
            .bind(estado)
            .bind(&mensaje)
            .bind(&paquetes)
            .bind(id_solicitud)
            .bind(id)
            .execute(&st.db)
            .await?
            .rows_affected();
            if filas == 0 {
                return Err(AppError::usuario("Solicitud desconocida."));
            }
            ok(mensaje)
        }

        "PeticionDescargaMasivaTercerosEntrada" => {
            let paquete = datos.attribute("IdPaquete").unwrap_or_default();
            let suyo: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sat_solicitudes WHERE usuario_id = $1 AND $2 = ANY(paquetes))",
            )
            .bind(id)
            .bind(paquete)
            .fetch_one(&st.db)
            .await?;
            if !suyo {
                return Err(AppError::usuario("Paquete desconocido."));
            }
            let respuesta = llamar(&st.http, DESCARGA, &format!("{ACCION}/IDescargaMasivaTercerosService/Descargar"), &envio.xml, Some(&token)).await?;
            let r = Document::parse(&respuesta).map_err(|_| sat_raro())?;
            let encabezado = elemento(&r, "respuesta").ok_or_else(sat_raro)?;
            if atributo(encabezado, "CodEstatus") != "5000" {
                return Err(AppError::usuario(&format!("El SAT no entregó el paquete: {}", atributo(encabezado, "Mensaje"))));
            }
            let base64: String = elemento(&r, "Paquete").and_then(|n| n.text()).unwrap_or_default().split_whitespace().collect();
            let zip = B64.decode(base64).map_err(|_| sat_raro())?;
            let cfdis = cfdi::leer_paquete(&zip).map_err(|e| AppError::usuario(&e))?;

            let mut tx = st.db.begin().await?;
            let mut nuevos = 0;
            for c in &cfdis {
                nuevos += sqlx::query(
                    "INSERT INTO cfdis (usuario_id, uuid, emitido, fecha, tipo, emisor_rfc, emisor_nombre, receptor_rfc,
                        receptor_nombre, uso_cfdi, metodo_pago, forma_pago, moneda, subtotal, total, xml)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                     ON CONFLICT DO NOTHING",
                )
                .bind(id)
                .bind(&c.uuid)
                .bind(c.emisor_rfc == rfc)
                .bind(c.fecha)
                .bind(&c.tipo)
                .bind(&c.emisor_rfc)
                .bind(&c.emisor_nombre)
                .bind(&c.receptor_rfc)
                .bind(&c.receptor_nombre)
                .bind(&c.uso_cfdi)
                .bind(&c.metodo_pago)
                .bind(&c.forma_pago)
                .bind(&c.moneda)
                .bind(c.subtotal)
                .bind(c.total)
                .bind(&c.xml)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            }
            sqlx::query(
                "UPDATE sat_solicitudes SET descargados = array_append(descargados, $1),
                    estado = CASE WHEN cardinality(paquetes) <= cardinality(descargados) + 1 THEN 'descargada' ELSE estado END
                 WHERE usuario_id = $2 AND $1 = ANY(paquetes) AND NOT $1 = ANY(descargados)",
            )
            .bind(paquete)
            .bind(id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            ok(format!("Paquete descargado: {} CFDI, {nuevos} nuevos.", cfdis.len()))
        }

        _ => Err(AppError::usuario("Operación desconocida.")),
    }
}

/// Estado de la solicitud según la verificación del SAT.
fn verificacion(cod: &str, estado: &str, codigo: &str, mensaje: &str, paquetes: usize) -> (&'static str, String) {
    if cod != "5000" {
        return ("solicitada", format!("El SAT no pudo verificar: {mensaje}"));
    }
    // 5004: no hay CFDI en el periodo; cuenta como terminada.
    if codigo == "5004" || (estado == "3" && paquetes == 0) {
        return ("descargada", "Sin CFDI en el periodo.".into());
    }
    match estado {
        "1" | "2" => ("solicitada", "En proceso en el SAT.".into()),
        "3" => ("lista", format!("Lista: {paquetes} paquete(s) por descargar.")),
        _ => ("error", format!("El SAT terminó la solicitud con error {codigo}: {mensaje}")),
    }
}

async fn llamar(http: &reqwest::Client, url: &str, accion: &str, xml: &str, token: Option<&str>) -> Result<String, AppError> {
    let mut peticion = http
        .post(url)
        .header("SOAPAction", accion)
        .header("Content-Type", r#"text/xml; charset="utf-8""#)
        .header("Accept", "text/xml")
        .body(xml.to_owned());
    if let Some(t) = token {
        peticion = peticion.header("Authorization", format!(r#"WRAP access_token="{t}""#));
    }
    let respuesta = peticion.send().await.map_err(|e| {
        eprintln!("SAT {url}: {e}");
        AppError(StatusCode::BAD_GATEWAY, "No se pudo conectar con el SAT; intenta más tarde.".into())
    })?;
    let estado = respuesta.status();
    let texto = respuesta.text().await?;
    if let Ok(doc) = Document::parse(&texto)
        && let Some(falla) = elemento(&doc, "faultstring").and_then(|n| n.text())
    {
        return Err(AppError(StatusCode::BAD_GATEWAY, format!("El SAT respondió: {falla}")));
    }
    if !estado.is_success() {
        eprintln!("SAT {url}: HTTP {estado}: {texto:.500}");
        return Err(AppError(StatusCode::BAD_GATEWAY, format!("El SAT respondió con error HTTP {estado}.")));
    }
    Ok(texto)
}

fn sat_raro() -> AppError {
    AppError(StatusCode::BAD_GATEWAY, "Respuesta inesperada del SAT.".into())
}

fn elemento<'a, 'i>(doc: &'a Document<'i>, nombre: &str) -> Option<Node<'a, 'i>> {
    doc.descendants().find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case(nombre))
}

fn atributo(nodo: Node, nombre: &str) -> String {
    nodo.attributes().find(|a| a.name().eq_ignore_ascii_case(nombre)).map(|a| a.value().to_string()).unwrap_or_default()
}

#[test]
fn estados_de_verificacion() {
    assert_eq!(verificacion("5000", "2", "5000", "", 0).0, "solicitada");
    assert_eq!(verificacion("5000", "3", "5000", "", 2), ("lista", "Lista: 2 paquete(s) por descargar.".into()));
    assert_eq!(verificacion("5000", "3", "5004", "", 0).0, "descargada");
    assert_eq!(verificacion("5000", "5", "5005", "Duplicada", 0).0, "error");
    assert_eq!(verificacion("300", "", "", "Token inválido", 0).0, "solicitada");
}
