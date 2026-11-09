//! Lectura de CFDI 3.3 / 4.0 y de los paquetes .zip que entrega el SAT.

use std::io::{Cursor, Read};

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::State, response::{IntoResponse, Redirect, Response}};
use rust_decimal::Decimal;
use time::{PrimitiveDateTime, macros::format_description};

use crate::{AppError, AppState, auth::Usuario};

#[derive(sqlx::FromRow)]
pub struct Fila {
    fecha: PrimitiveDateTime,
    emitido: bool,
    tipo: String,
    contraparte: String,
    uso_cfdi: String,
    total: Decimal,
    moneda: String,
    estado_sat: Option<String>,
}

impl Fila {
    pub fn estado_texto(&self) -> &str {
        self.estado_sat.as_deref().unwrap_or("—")
    }
}

impl Fila {
    pub fn fecha_texto(&self) -> String {
        self.fecha.format(format_description!("[day]/[month]/[year]")).unwrap_or_default()
    }

    pub fn tipo_texto(&self) -> &'static str {
        match self.tipo.as_str() {
            "I" => "Ingreso",
            "E" => "Egreso",
            "P" => "Pago",
            "N" => "Nómina",
            "T" => "Traslado",
            _ => "Otro",
        }
    }

    pub fn total_texto(&self) -> String {
        format!("${} {}", self.total.round_dp(2), self.moneda)
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "cfdi.html")]
pub struct Pagina {
    filas: Vec<Fila>,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Pagina, AppError> {
    // ponytail: últimos 200; paginar y filtrar por mes cuando llegue el informe mensual.
    let filas = sqlx::query_as(
        "SELECT fecha, emitido, tipo, uso_cfdi, total, moneda, estado_sat,
                CASE WHEN emitido THEN receptor_nombre || ' (' || receptor_rfc || ')'
                     ELSE emisor_nombre || ' (' || emisor_rfc || ')' END AS contraparte
         FROM cfdis WHERE usuario_id = $1 ORDER BY fecha DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    Ok(Pagina { filas })
}

// --- Validación contra el SAT (servicio público ConsultaCFDIService) ---

const SAT_CONSULTA: &str = "https://consultaqr.facturaelectronica.sat.gob.mx/ConsultaCFDIService.svc";

fn escapar(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Consulta el estado de un CFDI en el SAT (Vigente / Cancelado / No Encontrado).
async fn estado_sat(http: &reqwest::Client, emisor: &str, receptor: &str, total: &str, uuid: &str) -> Option<String> {
    let expr = escapar(&format!("?re={emisor}&rr={receptor}&tt={total}&id={uuid}"));
    let body = format!(
        "<soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:tem=\"http://tempuri.org/\">\
         <soapenv:Header/><soapenv:Body><tem:Consulta><tem:expresionImpresa>{expr}</tem:expresionImpresa></tem:Consulta></soapenv:Body></soapenv:Envelope>"
    );
    let t = http
        .post(SAT_CONSULTA)
        .header("Content-Type", "text/xml; charset=utf-8")
        .header("SOAPAction", "http://tempuri.org/IConsultaCFDIService/Consulta")
        .body(body)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    let ini = t.find("<a:Estado>")? + "<a:Estado>".len();
    let fin = t[ini..].find("</a:Estado>")?;
    Some(t[ini..ini + fin].to_string())
}

/// Valida todos los CFDI del usuario contra el SAT y guarda su estado.
pub async fn validar(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    let cfdis: Vec<(String, String, String, Decimal)> = sqlx::query_as(
        "SELECT uuid, emisor_rfc, receptor_rfc, total FROM cfdis WHERE usuario_id = $1",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    for (uuid, emisor, receptor, total) in cfdis {
        if let Some(estado) = estado_sat(&st.http, &emisor, &receptor, &total.to_string(), &uuid).await {
            sqlx::query("UPDATE cfdis SET estado_sat = $1, validado_en = now() WHERE usuario_id = $2 AND uuid = $3")
                .bind(&estado)
                .bind(id)
                .bind(&uuid)
                .execute(&st.db)
                .await?;
        }
        // ponytail: en serie con pausa breve para no saturar al SAT; un trabajo en segundo plano si crece.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
    Ok(Redirect::to("/cfdi").into_response())
}

pub struct Cfdi {
    pub uuid: String,
    pub fecha: PrimitiveDateTime,
    pub tipo: String,
    pub emisor_rfc: String,
    pub emisor_nombre: String,
    pub receptor_rfc: String,
    pub receptor_nombre: String,
    pub uso_cfdi: String,
    pub metodo_pago: String,
    pub forma_pago: String,
    pub moneda: String,
    pub subtotal: Decimal,
    pub total: Decimal,
    pub xml: String,
}

pub fn leer(xml: &str) -> Result<Cfdi, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let raiz = doc.root_element();
    if !raiz.has_tag_name("Comprobante") {
        return Err("no es un CFDI".into());
    }
    let hijo = |nombre: &str| raiz.descendants().find(|n| n.has_tag_name(nombre));
    let attr = |nodo: Option<roxmltree::Node>, nombre: &str| {
        nodo.and_then(|n| n.attribute(nombre)).unwrap_or_default().to_string()
    };
    let numero = |nombre: &str| {
        raiz.attribute(nombre).unwrap_or("0").parse::<Decimal>().map_err(|_| format!("{nombre} inválido"))
    };
    let (emisor, receptor) = (hijo("Emisor"), hijo("Receptor"));
    let fecha = raiz.attribute("Fecha").ok_or("sin Fecha")?;
    Ok(Cfdi {
        uuid: hijo("TimbreFiscalDigital").and_then(|t| t.attribute("UUID")).ok_or("sin timbre fiscal")?.to_uppercase(),
        fecha: PrimitiveDateTime::parse(fecha, format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]"))
            .map_err(|_| "Fecha inválida")?,
        tipo: attr(Some(raiz), "TipoDeComprobante"),
        emisor_rfc: attr(emisor, "Rfc").to_uppercase(),
        emisor_nombre: attr(emisor, "Nombre"),
        receptor_rfc: attr(receptor, "Rfc").to_uppercase(),
        receptor_nombre: attr(receptor, "Nombre"),
        uso_cfdi: attr(receptor, "UsoCFDI"),
        metodo_pago: attr(Some(raiz), "MetodoPago"),
        forma_pago: attr(Some(raiz), "FormaPago"),
        moneda: attr(Some(raiz), "Moneda"),
        subtotal: numero("SubTotal")?,
        total: numero("Total")?,
        xml: xml.to_string(),
    })
}

/// CFDI de un paquete .zip del SAT. Ignora archivos que no son CFDI.
pub fn leer_paquete(zip: &[u8]) -> Result<Vec<Cfdi>, String> {
    let mut archivo = zip::ZipArchive::new(Cursor::new(zip)).map_err(|e| format!("paquete inválido: {e}"))?;
    let mut cfdis = Vec::new();
    for i in 0..archivo.len() {
        let mut f = archivo.by_index(i).map_err(|e| e.to_string())?;
        if !f.name().is_ok_and(|n| n.to_lowercase().ends_with(".xml")) {
            continue;
        }
        let mut xml = String::new();
        if f.read_to_string(&mut xml).is_ok()
            && let Ok(c) = leer(xml.trim_start_matches('\u{feff}'))
        {
            cfdis.push(c);
        }
    }
    Ok(cfdis)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const CFDI: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<cfdi:Comprobante xmlns:cfdi="http://www.sat.gob.mx/cfd/4" xmlns:tfd="http://www.sat.gob.mx/TimbreFiscalDigital"
  Version="4.0" Fecha="2026-10-05T12:34:56" SubTotal="1000.00" Total="1160.00" Moneda="MXN"
  TipoDeComprobante="I" MetodoPago="PUE" FormaPago="03" Exportacion="01" LugarExpedicion="06000">
  <cfdi:Emisor Rfc="AAA010101AAA" Nombre="EMPRESA &amp; CIA" RegimenFiscal="601"/>
  <cfdi:Receptor Rfc="peña850101ab1" Nombre="JUAN PEÑA" UsoCFDI="G03" DomicilioFiscalReceptor="06000" RegimenFiscalReceptor="612"/>
  <cfdi:Complemento>
    <tfd:TimbreFiscalDigital Version="1.1" UUID="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee" FechaTimbrado="2026-10-05T12:35:00"/>
  </cfdi:Complemento>
</cfdi:Comprobante>"#;

    #[test]
    fn paquete() {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opciones = zip::write::SimpleFileOptions::default();
        zip.start_file("AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE.xml", opciones).unwrap();
        zip.write_all(CFDI.as_bytes()).unwrap();
        zip.start_file("otro.txt", opciones).unwrap();
        zip.write_all(b"no es cfdi").unwrap();
        zip.start_file("roto.xml", opciones).unwrap();
        zip.write_all(b"<a>").unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let cfdis = leer_paquete(&bytes).unwrap();
        assert_eq!(cfdis.len(), 1);
        let c = &cfdis[0];
        assert_eq!(c.uuid, "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE");
        assert_eq!(c.emisor_nombre, "EMPRESA & CIA");
        assert_eq!(c.receptor_rfc, "PEÑA850101AB1");
        assert_eq!((c.tipo.as_str(), c.uso_cfdi.as_str()), ("I", "G03"));
        assert_eq!(c.total, "1160.00".parse::<Decimal>().unwrap());
        assert_eq!(c.fecha.to_string(), "2026-10-05 12:34:56.0");
    }
}
