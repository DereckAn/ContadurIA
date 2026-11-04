//! e.firma (FIEL) del SAT en el navegador: abre el .cer y el .key, y firma los
//! mensajes del web service de Descarga Masiva. Se compila a WebAssembly; la llave
//! privada nunca sale del navegador.
//!
//! Los mensajes replican byte por byte los de phpcfdi/sat-ws-descarga-masiva
//! (ver pruebas). Al construir el XML ya canónico no hace falta un canonicalizador.

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use pkcs8::{DecodePrivateKey, DecodePublicKey, EncryptedPrivateKeyInfo, der::{Decode, Encode}};
use rsa::{
    BigUint, RsaPrivateKey, RsaPublicKey,
    pkcs1v15::SigningKey,
    signature::{SignatureEncoding, Signer},
};
use sha1::{Digest, Sha1};
use wasm_bindgen::prelude::*;
use x509_cert::{Certificate, name::Name};

const SOAP: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const DES: &str = "http://DescargaMasivaTerceros.sat.gob.mx";
const DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
const WSU: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";
const WSSE: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
const X509V3: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-x509-token-profile-1.0#X509v3";
const BASE64BIN: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary";

#[wasm_bindgen]
pub struct Efirma {
    llave: SigningKey<Sha1>,
    certificado: String,
    rfc: String,
    emisor: String,
    serie: String,
    vence_ms: f64,
}

#[wasm_bindgen]
impl Efirma {
    /// Abre la e.firma. Falla si la contraseña es incorrecta o si la llave no es la del certificado.
    #[wasm_bindgen(constructor)]
    pub fn new(cer: &[u8], key: &[u8], contrasena: &str) -> Result<Efirma, String> {
        let cert = Certificate::from_der(cer).map_err(|_| "El archivo .cer no es un certificado válido.")?;
        let cifrada = EncryptedPrivateKeyInfo::try_from(key).map_err(|_| "El archivo .key no es una llave del SAT.")?;
        let der = cifrada.decrypt(contrasena).map_err(|_| "Contraseña de la e.firma incorrecta.")?;
        let llave = RsaPrivateKey::from_pkcs8_der(der.as_bytes()).map_err(|_| "La llave no es RSA.")?;

        let tbs = &cert.tbs_certificate;
        let spki = tbs.subject_public_key_info.to_der().map_err(|e| e.to_string())?;
        let publica = RsaPublicKey::from_public_key_der(&spki).map_err(|_| "El certificado no es RSA.")?;
        if llave.to_public_key() != publica {
            return Err("El .key no corresponde a este .cer.".into());
        }

        // En la e.firma, x500UniqueIdentifier del titular es "RFC" o "RFC / RFC del representante".
        let rfc = atributos(&tbs.subject)
            .into_iter()
            .find(|(n, _)| n == "x500UniqueIdentifier")
            .and_then(|(_, v)| v.split('/').next().map(|r| r.trim().to_uppercase()))
            .ok_or("El certificado no tiene RFC.")?;

        Ok(Efirma {
            llave: SigningKey::<Sha1>::new(llave),
            certificado: B64.encode(cer),
            rfc,
            emisor: atributos(&tbs.issuer)
                .iter()
                .map(|(n, v)| format!("{}={}", rfc4514(n), rfc4514(v)))
                .collect::<Vec<_>>()
                .join(","),
            serie: BigUint::from_bytes_be(tbs.serial_number.as_bytes()).to_string(),
            vence_ms: tbs.validity.not_after.to_unix_duration().as_millis() as f64,
        })
    }

    #[wasm_bindgen(getter)]
    pub fn rfc(&self) -> String {
        self.rfc.clone()
    }

    /// Fecha de vencimiento del certificado, en milisegundos Unix (para `new Date()`).
    #[wasm_bindgen(getter)]
    pub fn vence(&self) -> f64 {
        self.vence_ms
    }

    /// Autenticación: firma un Timestamp. `creado`/`expira` como `2026-11-03T12:00:00.000Z`.
    pub fn autenticacion(&self, creado: &str, expira: &str, token_id: &str) -> String {
        let (creado, expira, token_id) = (xml(creado), xml(expira), xml(token_id));
        let tiempo = format!("<u:Created>{creado}</u:Created><u:Expires>{expira}</u:Expires>");
        let key_info = format!(
            r##"<KeyInfo><o:SecurityTokenReference><o:Reference URI="#{token_id}" ValueType="{X509V3}"/></o:SecurityTokenReference></KeyInfo>"##
        );
        let firma = self.firmar(&format!(r#"<u:Timestamp xmlns:u="{WSU}" u:Id="_0">{tiempo}</u:Timestamp>"#), "#_0", &key_info);
        format!(
            r#"<s:Envelope xmlns:s="{SOAP}" xmlns:u="{WSU}"><s:Header><o:Security xmlns:o="{WSSE}" s:mustUnderstand="1"><u:Timestamp u:Id="_0">{tiempo}</u:Timestamp><o:BinarySecurityToken u:Id="{token_id}" ValueType="{X509V3}" EncodingType="{BASE64BIN}">{}</o:BinarySecurityToken>{firma}</o:Security></s:Header><s:Body><Autentica xmlns="http://DescargaMasivaTerceros.gob.mx"/></s:Body></s:Envelope>"#,
            self.certificado
        )
    }

    /// Solicitud de CFDI (XML completos) `emitidos` o `recibidos` entre `inicio` y `fin` (`2026-01-01T00:00:00`, hora de México).
    pub fn solicitud(&self, tipo: &str, inicio: &str, fin: &str) -> Result<String, String> {
        let rfc = self.rfc.as_str();
        match tipo {
            "emitidos" => Ok(self.solicitud_con(
                "SolicitaDescargaEmitidos",
                &[("EstadoComprobante", "Todos"), ("FechaFinal", fin), ("FechaInicial", inicio),
                  ("RfcEmisor", rfc), ("RfcSolicitante", rfc), ("TipoSolicitud", "CFDI")],
            )),
            // El SAT rechaza recibidos en XML que incluyan cancelados: sólo vigentes.
            "recibidos" => Ok(self.solicitud_con(
                "SolicitaDescargaRecibidos",
                &[("EstadoComprobante", "Vigente"), ("FechaFinal", fin), ("FechaInicial", inicio),
                  ("RfcReceptor", rfc), ("RfcSolicitante", rfc), ("TipoSolicitud", "CFDI")],
            )),
            _ => Err(format!("Tipo de solicitud desconocido: {tipo}")),
        }
    }

    pub fn verificacion(&self, id_solicitud: &str) -> String {
        let atributos = format!(r#"IdSolicitud="{}" RfcSolicitante="{}""#, xml(id_solicitud), xml(&self.rfc));
        self.envolver("VerificaSolicitudDescarga", "solicitud", &atributos, "")
    }

    pub fn descarga(&self, id_paquete: &str) -> String {
        let atributos = format!(r#"IdPaquete="{}" RfcSolicitante="{}""#, xml(id_paquete), xml(&self.rfc));
        self.envolver("PeticionDescargaMasivaTercerosEntrada", "peticionDescarga", &atributos, "")
    }
}

impl Efirma {
    /// `atributos` deben ir en orden alfabético (así los firma el SAT).
    fn solicitud_con(&self, nodo: &str, atributos: &[(&str, &str)]) -> String {
        let atributos = atributos
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(n, v)| format!(r#"{n}="{}""#, xml(v)))
            .collect::<Vec<_>>()
            .join(" ");
        self.envolver(nodo, "solicitud", &atributos, "")
    }

    /// Mensaje con firma envuelta dentro de `<des:{hijo}>`.
    fn envolver(&self, nodo: &str, hijo: &str, atributos: &str, extra: &str) -> String {
        let a_digerir = format!(r#"<des:{nodo} xmlns:des="{DES}"><des:{hijo} {atributos}>{extra}</des:{hijo}></des:{nodo}>"#);
        let firma = self.firmar(&a_digerir, "", &self.key_info());
        format!(
            r#"<s:Envelope xmlns:s="{SOAP}" xmlns:des="{DES}" xmlns:xd="{DSIG}"><s:Header/><s:Body><des:{nodo}><des:{hijo} {atributos}>{extra}{firma}</des:{hijo}></des:{nodo}></s:Body></s:Envelope>"#
        )
    }

    /// XMLDSig con RSA-SHA1 sobre `a_digerir`, que ya debe estar en forma canónica exclusiva.
    fn firmar(&self, a_digerir: &str, uri: &str, key_info: &str) -> String {
        let digest = B64.encode(Sha1::digest(a_digerir.as_bytes()));
        let signed_info = format!(
            r#"<SignedInfo xmlns="{DSIG}"><CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></CanonicalizationMethod><SignatureMethod Algorithm="http://www.w3.org/2000/09/xmldsig#rsa-sha1"></SignatureMethod><Reference URI="{uri}"><Transforms><Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"></Transform></Transforms><DigestMethod Algorithm="http://www.w3.org/2000/09/xmldsig#sha1"></DigestMethod><DigestValue>{digest}</DigestValue></Reference></SignedInfo>"#
        );
        let valor = B64.encode(self.llave.sign(signed_info.as_bytes()).to_bytes());
        let signed_info = signed_info.replacen(&format!(r#"<SignedInfo xmlns="{DSIG}">"#), "<SignedInfo>", 1);
        format!(r#"<Signature xmlns="{DSIG}">{signed_info}<SignatureValue>{valor}</SignatureValue>{key_info}</Signature>"#)
    }

    fn key_info(&self) -> String {
        format!(
            "<KeyInfo><X509Data><X509IssuerSerial><X509IssuerName>{}</X509IssuerName><X509SerialNumber>{}</X509SerialNumber></X509IssuerSerial><X509Certificate>{}</X509Certificate></X509Data></KeyInfo>",
            xml(&self.emisor),
            self.serie,
            self.certificado
        )
    }
}

/// Atributos de un nombre X.500 en el orden del certificado, con los nombres cortos de OpenSSL.
fn atributos(nombre: &Name) -> Vec<(String, String)> {
    nombre
        .0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .map(|a| {
            let oid = a.oid.to_string();
            let corto = match oid.as_str() {
                "2.5.4.3" => "CN",
                "2.5.4.5" => "serialNumber",
                "2.5.4.6" => "C",
                "2.5.4.7" => "L",
                "2.5.4.8" => "ST",
                "2.5.4.9" => "street",
                "2.5.4.10" => "O",
                "2.5.4.11" => "OU",
                "2.5.4.17" => "postalCode",
                "2.5.4.41" => "name",
                "2.5.4.45" => "x500UniqueIdentifier",
                "1.2.840.113549.1.9.1" => "emailAddress",
                "1.2.840.113549.1.9.2" => "unstructuredName",
                _ => oid.as_str(),
            };
            (corto.to_string(), String::from_utf8_lossy(a.value.value()).into_owned())
        })
        .collect()
}

/// Escapado RFC 4514 igual al de phpcfdi/credentials.
fn rfc4514(valor: &str) -> String {
    let mut s = String::new();
    let n = valor.chars().count();
    for (i, c) in valor.chars().enumerate() {
        match c {
            ' ' if i == 0 || i == n - 1 => s.push_str("\\20"),
            '#' if i == 0 => s.push_str("\\22"),
            '\\' => s.push_str("\\5C"),
            '"' => s.push_str("\\22"),
            '+' => s.push_str("\\2b"),
            ',' => s.push_str("\\2c"),
            ';' => s.push_str("\\3b"),
            '<' => s.push_str("\\3c"),
            '=' => s.push_str("\\3d"),
            '>' => s.push_str("\\3e"),
            _ => s.push(c),
        }
    }
    s
}

fn xml(texto: &str) -> String {
    texto.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn efirma() -> Efirma {
        Efirma::new(include_bytes!("../pruebas/EKU9003173C9.cer"), include_bytes!("../pruebas/EKU9003173C9.key"), "12345678a")
            .unwrap()
    }

    /// Las pruebas de phpcfdi guardan los XML con sangría y a veces `<x/>` en lugar de `<x></x>`
    /// (equivalentes en XML); se comparan sin sangría y con elementos vacíos cerrados igual.
    fn compacto(xml: &str) -> String {
        let mut s: String = xml.lines().filter(|l| !l.starts_with("<?xml")).map(str::trim).collect();
        while let Some(i) = s.find("></") {
            let cierre = i + 1 + s[i + 1..].find('>').unwrap(); // el '>' de "</nombre>"
            s.replace_range(i..=cierre, "/>");
        }
        s
    }

    fn igual(mio: String, referencia: &str) {
        assert_eq!(compacto(&mio), compacto(referencia));
    }

    #[test]
    fn mensajes_identicos_a_la_referencia() {
        let e = efirma();
        assert_eq!(e.rfc(), "EKU9003173C9");

        igual(
            e.autenticacion("2019-08-01T03:38:19.000Z", "2019-08-01T03:43:19.000Z", "uuid-cf6c80fb-00ae-44c0-af56-54ec65decbaa-1"),
            include_str!("../pruebas/autenticacion.xml")
        );
        igual(
            e.solicitud_con(
                "SolicitaDescargaEmitidos",
                &[("EstadoComprobante", "Todos"), ("FechaFinal", "2019-01-01T00:04:00"), ("FechaInicial", "2019-01-01T00:00:00"),
                  ("RfcEmisor", "EKU9003173C9"), ("RfcSolicitante", "EKU9003173C9"), ("TipoSolicitud", "Metadata")],
            ),
            include_str!("../pruebas/solicitud-emitidos.xml")
        );
        igual(
            e.solicitud_con(
                "SolicitaDescargaRecibidos",
                &[("Complemento", "nomina12"), ("EstadoComprobante", "Vigente"), ("FechaFinal", "2019-01-01T00:04:00"),
                  ("FechaInicial", "2019-01-01T00:00:00"), ("RfcACuentaTerceros", "XXX01010199A"), ("RfcEmisor", "AAA010101AAA"),
                  ("RfcReceptor", "EKU9003173C9"), ("RfcSolicitante", "EKU9003173C9"), ("TipoComprobante", "N"), ("TipoSolicitud", "CFDI")],
            ),
            include_str!("../pruebas/solicitud-recibidos.xml")
        );
        igual(e.verificacion("3f30a4e1-af73-4085-8991-e4d97eef16bd"), include_str!("../pruebas/verificacion.xml"));
        igual(e.descarga("4e80345d-917f-40bb-a98f-4a73939343c5_01"), include_str!("../pruebas/descarga.xml"));
    }

    #[test]
    fn errores() {
        let cer = include_bytes!("../pruebas/EKU9003173C9.cer");
        let key = include_bytes!("../pruebas/EKU9003173C9.key");
        assert_eq!(Efirma::new(cer, key, "otra").err().unwrap(), "Contraseña de la e.firma incorrecta.");
        assert!(Efirma::new(key, key, "12345678a").is_err());
        assert!(efirma().solicitud("otros", "", "").is_err());
    }
}
