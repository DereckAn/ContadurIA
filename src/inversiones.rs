//! Estados de cuenta de inversión. Se suben por /banco junto con los del banco: si el PDF
//! es de inversión, se lee aquí y se guarda su resumen del mes por contrato. Formatos:
//! - GBM (casa de bolsa, pesos): "RESUMEN DEL PORTAFOLIO".
//! - GBM+ USD (DriveWealth, dólares): "Valuation Summary". Sus montos van en USD.
//!
//! Se usa `pdftotext -raw`: los PDF exportados desde el iPhone salen con espacios dentro de
//! palabras y números en modo -layout ("1 7,0 3 8.5 8"); en -raw salen limpios.

use std::str::FromStr;

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::State;
use rust_decimal::Decimal;
use time::{Date, Month};
use uuid::Uuid;

use crate::{AppError, AppState, auth::Usuario, gastos::pesos};

const MESES: [&str; 12] = [
    "ENERO", "FEBRERO", "MARZO", "ABRIL", "MAYO", "JUNIO",
    "JULIO", "AGOSTO", "SEPTIEMBRE", "OCTUBRE", "NOVIEMBRE", "DICIEMBRE",
];

#[derive(Debug, PartialEq)]
pub struct EstadoGbm {
    contrato: String,
    moneda: &'static str,
    periodo: Date,
    valor_inicial: Decimal,
    valor_final: Decimal,
    entradas: Decimal,
    salidas: Decimal,
    intereses: Decimal,
    isr_retenido: Decimal,
    dolares_usd: Decimal,
    dolares_mxn: Decimal,
}

/// ¿Es un estado de inversión? Se revisa sin espacios porque -layout los mete dentro de las palabras.
pub fn es_inversion(texto: &str) -> bool {
    let c: String = texto.chars().filter(|c| !c.is_whitespace()).collect();
    (c.contains("RESUMENDELPORTAFOLIO") && c.contains("Contrato:"))
        || (c.contains("DriveWealth") && c.contains("ValuationSummary"))
}

/// "17,041.92" → 17041.92; "(419.78)" → -419.78; "$260.73" → 260.73. Exige punto decimal
/// (descarta folios).
fn monto(t: &str) -> Option<Decimal> {
    let t = t.replace('$', "");
    let t = t.as_str();
    let (neg, t) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(x) => (true, x),
        None => (false, t),
    };
    if !t.contains('.') {
        return None;
    }
    let d = Decimal::from_str(&t.replace(',', "")).ok()?;
    Some(if neg { -d } else { d })
}

/// Primera línea cuyo texto sin espacios contiene la etiqueta (también sin espacios).
fn linea<'a>(texto: &'a str, etiqueta: &str) -> Option<&'a str> {
    let e: String = etiqueta.chars().filter(|c| !c.is_whitespace()).collect();
    texto.lines().find(|l| l.chars().filter(|c| !c.is_whitespace()).collect::<String>().contains(&e))
}

fn montos(l: &str) -> Vec<Decimal> {
    l.split_whitespace().filter_map(monto).collect()
}

/// Etiqueta de la columna derecha del resumen: su monto es el último de la línea.
fn ultimo(texto: &str, etiqueta: &str) -> Option<Decimal> {
    montos(linea(texto, etiqueta)?).last().copied()
}

/// "DEL 1 AL 31 DE OCTUBRE DE 2026" → 2026-10-01.
fn periodo(texto: &str) -> Option<Date> {
    let l = texto.lines().find(|l| l.starts_with("DEL ") && l.contains(" AL "))?;
    let palabras: Vec<&str> = l.split_whitespace().collect();
    let mes = palabras.iter().find_map(|p| MESES.iter().position(|m| m == p))?;
    let anio: i32 = palabras.last()?.parse().ok()?;
    Date::from_calendar_date(anio, Month::try_from(mes as u8 + 1).ok()?, 1).ok()
}

/// Lee el texto (-raw) de un estado mensual de inversión.
pub fn parsear(texto: &str) -> Option<EstadoGbm> {
    if texto.contains("DriveWealth") { parsear_usd(texto) } else { parsear_gbm(texto) }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];

/// GBM+ USD (DriveWealth). "Valuation Summary": la primera cifra de cada renglón es la del mes.
/// No trae ISR mexicano retenido: es un broker de EE. UU.
fn parsear_usd(texto: &str) -> Option<EstadoGbm> {
    let cifra = |etiqueta: &str| {
        texto.lines().find(|l| l.starts_with(etiqueta)).and_then(|l| l[etiqueta.len()..].split_whitespace().next()).and_then(monto)
    };
    let contrato = texto.split("Account Number:").nth(1)?.split_whitespace().next()?.to_string();
    // "October 01, 2026 - October 31, 2026"
    let l = texto.lines().find(|l| MONTHS.iter().any(|m| l.starts_with(m)) && l.contains(" - "))?;
    let mes = MONTHS.iter().position(|m| l.starts_with(m))?;
    let anio: i32 = l.split(", ").nth(1)?.get(..4)?.parse().ok()?;
    Some(EstadoGbm {
        contrato,
        moneda: "USD",
        periodo: Date::from_calendar_date(anio, Month::try_from(mes as u8 + 1).ok()?, 1).ok()?,
        valor_inicial: cifra("Beginning Account Value ")?,
        valor_final: cifra("Ending Account Value ")?,
        entradas: cifra("Deposits ")?,
        salidas: cifra("Withdrawals ")?,
        intereses: cifra("Dividend & Interest ")?,
        isr_retenido: Decimal::ZERO,
        dolares_usd: Decimal::ZERO,
        dolares_mxn: Decimal::ZERO,
    })
}

/// GBM casa de bolsa (pesos).
fn parsear_gbm(texto: &str) -> Option<EstadoGbm> {
    let contrato = linea(texto, "Contrato:")?
        .split("Contrato:")
        .nth(1)?
        .split_whitespace()
        .next()?
        .to_string();
    let valor = montos(linea(texto, "VALOR DEL PORTAFOLIO")?);

    // Compras de dólares en el desglose de movimientos:
    // "15/16 40480098 Co m pra D OLARES 261 17.258900 0.00 0.00 0.00 4,499.91 -4,408.40"
    // → títulos (USD) después de "OLARES"; el neto en pesos es el penúltimo número.
    let (mut dolares_usd, mut dolares_mxn) = (Decimal::ZERO, Decimal::ZERO);
    for l in texto.lines().filter(|l| l.replace(' ', "").contains("CompraDOLARES")) {
        let t: Vec<&str> = l.split_whitespace().collect();
        let usd = t.iter().position(|p| p.ends_with("OLARES")).and_then(|i| t.get(i + 1)).and_then(|x| Decimal::from_str(&x.replace(',', "")).ok());
        let mxn = t.len().checked_sub(2).and_then(|i| monto(t[i]));
        if let (Some(u), Some(m)) = (usd, mxn) {
            dolares_usd += u;
            dolares_mxn += m;
        }
    }

    Some(EstadoGbm {
        contrato,
        moneda: "MXN",
        periodo: periodo(texto)?,
        valor_inicial: *valor.first()?,
        valor_final: *valor.get(1)?,
        entradas: ultimo(texto, "ENTRADAS DE EFECTIVO")?,
        salidas: ultimo(texto, "SALIDAS DE EFECTIVO")?,
        intereses: ultimo(texto, "DERECHOS PATRIMONIALES")?,
        // "TOTALES: 4.60 0.00 ..." — la primera columna es el ISR retenido. En mayúsculas para
        // no confundir con "Totales:" de la tabla de comisiones.
        isr_retenido: *montos(linea(texto, "TOTALES:")?).first()?,
        dolares_usd,
        dolares_mxn,
    })
}

/// Lee un PDF de inversión y guarda (o reemplaza) el resumen de ese contrato y mes.
pub async fn guardar(st: &AppState, id: Uuid, archivo: &str, pdf: &[u8]) -> Result<String, AppError> {
    let texto = crate::banco::texto_pdf(pdf, "-raw")?;
    let e = parsear(&texto).ok_or_else(|| AppError::usuario("parece un estado de inversión, pero no se reconoció el formato"))?;
    sqlx::query(
        "INSERT INTO inversiones (usuario_id, contrato, periodo, valor_inicial, valor_final, entradas, salidas,
                                  intereses, isr_retenido, dolares_usd, dolares_mxn, archivo, moneda)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
         ON CONFLICT (usuario_id, contrato, periodo) DO UPDATE SET
            valor_inicial = EXCLUDED.valor_inicial, valor_final = EXCLUDED.valor_final,
            entradas = EXCLUDED.entradas, salidas = EXCLUDED.salidas, intereses = EXCLUDED.intereses,
            isr_retenido = EXCLUDED.isr_retenido, dolares_usd = EXCLUDED.dolares_usd,
            dolares_mxn = EXCLUDED.dolares_mxn, archivo = EXCLUDED.archivo, moneda = EXCLUDED.moneda",
    )
    .bind(id)
    .bind(&e.contrato)
    .bind(e.periodo)
    .bind(e.valor_inicial)
    .bind(e.valor_final)
    .bind(e.entradas)
    .bind(e.salidas)
    .bind(e.intereses)
    .bind(e.isr_retenido)
    .bind(e.dolares_usd)
    .bind(e.dolares_mxn)
    .bind(archivo)
    .bind(e.moneda)
    .execute(&st.db)
    .await?;
    Ok(format!("{} ({}) {}/{}", e.contrato, e.moneda, e.periodo.month() as u8, e.periodo.year()))
}

// ---------- Página ----------

#[derive(sqlx::FromRow)]
pub struct Fila {
    contrato: String,
    moneda: String,
    periodo: Date,
    valor_inicial: Decimal,
    valor_final: Decimal,
    entradas: Decimal,
    salidas: Decimal,
    intereses: Decimal,
    isr_retenido: Decimal,
    dolares_usd: Decimal,
}

impl Fila {
    pub fn mes(&self) -> String {
        let m = MESES[self.periodo.month() as usize - 1];
        format!("{}{} {}", &m[..1], m[1..].to_lowercase(), self.periodo.year())
    }
    fn dinero(&self, n: Decimal) -> String {
        if self.moneda == "USD" { dolares(n) } else { pesos(n) }
    }
    pub fn valor_inicial(&self) -> String { self.dinero(self.valor_inicial) }
    pub fn valor_final(&self) -> String { self.dinero(self.valor_final) }
    pub fn entradas(&self) -> String { self.dinero(self.entradas) }
    pub fn salidas(&self) -> String { self.dinero(self.salidas) }
    pub fn intereses(&self) -> String {
        if self.moneda == "USD" { dolares(self.intereses) } else { centavos(self.intereses) }
    }
    pub fn isr(&self) -> String {
        if self.moneda == "USD" { "—".into() } else { centavos(self.isr_retenido) }
    }
    pub fn dolares(&self) -> String {
        if self.dolares_usd.is_zero() { String::new() } else { format!("{} USD", self.dolares_usd.normalize()) }
    }
}

/// Montos chicos (intereses, ISR) con centavos.
fn centavos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

fn dolares(n: Decimal) -> String {
    format!("US${:.2}", n.round_dp(2))
}

#[derive(Template, WebTemplate)]
#[template(path = "inversiones.html")]
pub struct Pagina {
    filas: Vec<Fila>,
    valor_actual: String,
    aportado: String,
    intereses: String,
    isr: String,
    dolares_usd: String,
    dolares_mxn: String,
    hay_dolares: bool,
    // Cuenta en dólares, si se subió su estado.
    valor_usd: String,
    depositado_usd: String,
    hay_cuenta_usd: bool,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Pagina, AppError> {
    // Los contratos que no se han usado (todo en cero) no se muestran.
    let filas: Vec<Fila> = sqlx::query_as(
        "SELECT contrato, moneda, periodo, valor_inicial, valor_final, entradas, salidas, intereses, isr_retenido, dolares_usd
         FROM inversiones
         WHERE usuario_id = $1 AND (valor_inicial <> 0 OR valor_final <> 0 OR entradas <> 0 OR salidas <> 0)
         ORDER BY periodo DESC, moneda, contrato",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let (dolares_usd, dolares_mxn): (Decimal, Decimal) = sqlx::query_as(
        "SELECT COALESCE(sum(dolares_usd), 0), COALESCE(sum(dolares_mxn), 0) FROM inversiones WHERE usuario_id = $1 AND moneda = 'MXN'",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    // Valor actual = último mes de cada contrato, por moneda (pesos y dólares nunca se suman).
    let actual = |moneda: &'static str| {
        sqlx::query_scalar::<_, Decimal>(
            "SELECT COALESCE(sum(valor_final), 0) FROM (
                SELECT DISTINCT ON (contrato) valor_final FROM inversiones
                WHERE usuario_id = $1 AND moneda = $2 ORDER BY contrato, periodo DESC) u",
        )
        .bind(id)
        .bind(moneda)
        .fetch_one(&st.db)
    };
    let valor_actual = actual("MXN").await?;
    let valor_usd = actual("USD").await?;
    let suma = |moneda: &str, f: fn(&Fila) -> Decimal| filas.iter().filter(|x| x.moneda == moneda).map(f).sum::<Decimal>();
    let hay_cuenta_usd = filas.iter().any(|x| x.moneda == "USD");
    Ok(Pagina {
        valor_actual: pesos(valor_actual),
        aportado: pesos(suma("MXN", |f| f.entradas)),
        intereses: centavos(suma("MXN", |f| f.intereses)),
        isr: centavos(suma("MXN", |f| f.isr_retenido)),
        valor_usd: dolares(valor_usd),
        depositado_usd: dolares(suma("USD", |f| f.entradas)),
        hay_cuenta_usd,
        hay_dolares: !dolares_usd.is_zero(),
        dolares_usd: dolares_usd.normalize().to_string(),
        dolares_mxn: pesos(dolares_mxn),
        filas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Extracto con el formato real de `pdftotext -raw` de un estado de GBM (datos ficticios).
    const OCTUBRE: &str = "\
PORTAFOLIO AL 30-SEP-26 AL 30-OCT-26 PORTAFOLIO M OVIMIENTOS DEL 1 AL 31 DE OCTUBRE DE 2026
DEUDA 0.00 17,038.58 99.98 ENTRADAS DE EFECTIVO 22,000.00
RENTA VARIABLE 0.00 0.00 0.00 SALIDAS DE EFECTIVO 5,057.77
CREDITOS DE MARGEN 0.00 0.00 0.00 DERECHOS PATRIM ONIALES 20.74
VALOR DEL PORTAFOLIO \u{232b} 83.55 17,041.92 100.00 SALDO NETO DE M OVIMIENTOS 16,962.97
RESUMEN DEL PORTAFOLIO
02/05 41733826 Co m pra D OLARES 276 18.297420 0.00 0.00 0.00 5,057.77 -5,052.02
05/05 2019527724 Venci m ien to de Reporto BI 270318 524 9.693687 4.00 3 0.00 1.69 0.38 5,079.12 27.10
T OTALES: 4.60 0.00 0.00 0.00 0.00 0.00
Totales: 0.00 0.00
FULANO DE TAL Contrato: GV00XX01 RFC: XAXX010101000
Periodo:
DEL 1 AL 31 DE OCTUBRE DE 2026
";

    #[test]
    fn lee_estado_gbm() {
        let e = parsear(OCTUBRE).expect("debe parsear");
        let d = |s: &str| Decimal::from_str(s).unwrap();
        assert_eq!(e, EstadoGbm {
            contrato: "GV00XX01".into(),
            moneda: "MXN",
            periodo: Date::from_calendar_date(2026, Month::October, 1).unwrap(),
            valor_inicial: d("83.55"),
            valor_final: d("17041.92"),
            entradas: d("22000.00"),
            salidas: d("5057.77"),
            intereses: d("20.74"),
            isr_retenido: d("4.60"),
            dolares_usd: d("276"),
            dolares_mxn: d("5057.77"),
        });
    }

    #[test]
    fn detecta_inversion_aun_con_espacios_de_layout() {
        assert!(es_inversion("R ES U M E N D EL P O R T A F O LI O\nC o n t ra t o: GV 4 3I H 0 1"));
        assert!(es_inversion("Valuation Summary This Period\nClearing by DriveWealth, LLC"));
        assert!(!es_inversion("Detalle de Movimientos CARGOS ABONOS"));
        assert_eq!(monto("(419.78)"), Some(Decimal::from_str("-419.78").unwrap()));
        assert_eq!(monto("40480098"), None);
    }

    #[test]
    fn lee_estado_usd() {
        let texto = "\
Valuation Summary This Period This Year
Beginning Account Value $574.37 0.00
Deposits $276.42 $851.85
Dividend & Interest $0.00 $0.00
Withdrawals $0.00 $0.00
Ending Account Value $780.29 $780.29
Clearing and execution services provided by DriveWealth, LLC member FINRA and SIPC.
Account Number:GBMP-001-XX0001 Account Name: FULANO DE TAL
October 01, 2026 - October 31, 2026
Valuation Summaryprovides a summary which reflects the total value. Ending Account Value is explained here.
";
        let e = parsear(texto).expect("debe parsear");
        assert_eq!(e.contrato, "GBMP-001-XX0001");
        assert_eq!(e.moneda, "USD");
        assert_eq!(e.periodo, Date::from_calendar_date(2026, Month::October, 1).unwrap());
        assert_eq!(e.valor_inicial, Decimal::from_str("574.37").unwrap());
        assert_eq!(e.valor_final, Decimal::from_str("780.29").unwrap());
        assert_eq!(e.entradas, Decimal::from_str("276.42").unwrap());
        assert_eq!(e.isr_retenido, Decimal::ZERO);
    }

    // Revisión manual con PDFs reales: GBM_PDF=/ruta.pdf cargo test real_gbm -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_gbm() {
        for ruta in std::env::var("GBM_PDF").expect("define GBM_PDF").split(':') {
            let texto = crate::banco::texto_pdf(&std::fs::read(ruta).unwrap(), "-raw").ok().unwrap();
            println!("{ruta}: {:?}", parsear(&texto));
        }
    }
}
