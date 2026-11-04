//! Lectura de CFDI 3.3 / 4.0 y de los paquetes .zip que entrega el SAT.

use std::io::{Cursor, Read};

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
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
        "SELECT fecha, emitido, tipo, uso_cfdi, total, moneda,
                CASE WHEN emitido THEN receptor_nombre || ' (' || receptor_rfc || ')'
                     ELSE emisor_nombre || ' (' || emisor_rfc || ')' END AS contraparte
         FROM cfdis WHERE usuario_id = $1 ORDER BY fecha DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    Ok(Pagina { filas })
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
