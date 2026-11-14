//! ¿En qué se va mi dinero? Tablero de gastos a partir de los movimientos del banco.
//!
//! Cada cargo se agrupa por comercio (la descripción del banco sin folios) y se le da una
//! categoría: primero la que el usuario eligió o la IA ya decidió (tabla categorias_comercio),
//! luego reglas por palabra clave, y si nada aplica, "Otros" hasta que se clasifique con IA.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::State, response::Redirect};
use axum_extra::extract::Form;
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use time::Date;
use uuid::Uuid;

use crate::{AppError, AppState, auth::Usuario};

pub const CATEGORIAS: [&str; 17] = [
    "Comida", "Súper", "Tiendas de conveniencia", "Transporte", "Compras en línea", "Suscripciones",
    "Videojuegos", "Servicios", "Hogar", "Ropa", "Cuidado personal", "Salud y deporte", "Efectivo",
    "Impuestos", "Transferencias", "Inversión", "Otros",
];

/// Salidas de dinero que no son consumo: se muestran aparte y no cuentan como gasto.
pub const NO_CONSUMO: [&str; 2] = ["Transferencias", "Inversión"];

/// Reglas por palabra clave, en orden: gana la primera que aparezca en el comercio.
// ponytail: lista a mano sacada de estados reales; lo que no cubra lo resuelve la IA o el usuario.
const REGLAS: &[(&str, &[&str])] = &[
    ("Efectivo", &["RETIRO"]),
    ("Inversión", &["GBM", "FONDOS INVERSION", "CETES", "INVERSION"]),
    ("Transferencias", &["SPEI ENVIADO", "PAGO CUENTA DE TERCERO", "TRASPASO", "TRANSFERENCIA"]),
    ("Suscripciones", &[
        "AMAZON PRIME", "TIDAL", "SPOTIFY", "NETFLIX", "CLAUDE", "MOONSHOT", "RAILWAY", "ONLYFANS",
        "KASPERSKY", "DISNEY", "HBO", "YOUTUBE", "APPLE.COM", "ICLOUD", "CHATGPT", "OPENAI",
        "TWICH", "TWITCH", "CRUNCHYROLL", "PARAMOUNT", "VIX",
    ]),
    ("Videojuegos", &["STEAM", "PLAYSTATION", "ENEBA", "XBOX", "NINTENDO", "EPIC GAMES"]),
    ("Comida", &[
        "EATS", "RAPPI", "DIDI FOOD", "DOMINO", "BURGER", "KFC", "CARLS", "PIZZA", "TACO", "REST ",
        "HAMBURGUESA", "HABURGUESA", "COFFE", "CAFE", "STARBUCKS", "WING", "TORTA", "SNACK",
        "LITTLE CAESARS", "MCDONALD", "SUSHI",
    ]),
    ("Transporte", &[
        "UBER", "UBR*", "DIDI", "OVNI BUS", "OMNIBUS", "AUTOBUS", "PASE,", "PASE ", "TRANSPO",
        "GASOLIN", "PEMEX", "ESTACIONAMIENTO", "CABIFY",
    ]),
    ("Tiendas de conveniencia", &["7 ELEVEN", "OXXO", "CIRCULO K", "KIOSKO"]),
    ("Súper", &[
        "WAL MART", "WALMART", "SORIANA", "CHEDRAUI", "AURRERA", "COSTCO", "SAMS", "ABARROTES",
        "SUPERAMA", "LA COMER", "CITY MARKET",
    ]),
    ("Compras en línea", &[
        "AMAZON", "ALIEXPRESS", "MERCADOLIBRE", "DEPOP", "ALIBABA", "THOMANN", "SHEIN", "TEMU", "ORDER",
    ]),
    ("Servicios", &["TELMEX", "MEGACABLE", "TELCEL", "IZZI", "TOTALPLAY", "CFE", "RECARGAS", "AT&T", "MOVISTAR"]),
    ("Hogar", &["HOME DEPOT", "COMEX", "FERRETER"]),
    ("Ropa", &["BERSHKA", "ITX", "ZARA", "PULL&BEAR", "H&M", "LIVERPOOL", "STRADIVARIUS"]),
    ("Cuidado personal", &["BARBER", "SALON DE BELLEZA", "MAKEUP", "ESTETICA"]),
    ("Salud y deporte", &["FARMACIA", "FIT ", "GYM", "DOCTOR", "HOSPITAL", "LABORATORIO"]),
    ("Impuestos", &["IMPUESTO", "TESORERIA"]),
];

/// Nombre del comercio: la descripción en mayúsculas sin folios ni números de orden.
/// "AMAZON PRIME*JE3DQ1393" → "AMAZON PRIME"; "DOMINO'S 7501" → "DOMINO'S".
pub fn comercio(desc: &str) -> String {
    let mut s = desc.trim().to_uppercase();
    if let Some(i) = s.rfind('*') {
        if s[i + 1..].chars().any(|c| c.is_ascii_digit()) {
            s.truncate(i);
        }
    }
    let mut palabras: Vec<&str> = s.split_whitespace().collect();
    while palabras.len() > 1 && palabras.last().is_some_and(|p| p.chars().any(|c| c.is_ascii_digit())) {
        palabras.pop();
    }
    palabras.join(" ").trim_end_matches('*').trim().to_string()
}

fn por_palabra(comercio: &str) -> Option<&'static str> {
    if comercio.ends_with("CASH") {
        return Some("Efectivo"); // retiro de efectivo en caja (cashback)
    }
    REGLAS.iter().find(|(_, claves)| claves.iter().any(|k| comercio.contains(k))).map(|(c, _)| *c)
}

/// Categoría de un movimiento a partir de su descripción del banco.
pub fn categoria(descripcion: &str, guardadas: &HashMap<String, String>) -> String {
    categoria_de(&comercio(descripcion), guardadas)
}

fn categoria_de(comercio: &str, guardadas: &HashMap<String, String>) -> String {
    guardadas
        .get(comercio)
        .cloned()
        .or_else(|| por_palabra(comercio).map(str::to_string))
        .unwrap_or_else(|| "Otros".into())
}

// ---------- Cálculo del tablero (puro, sin base de datos) ----------

#[derive(sqlx::FromRow)]
pub struct Mov {
    fecha: Date,
    descripcion: String,
    cargo: Option<Decimal>,
    abono: Option<Decimal>,
}

pub fn pesos(n: Decimal) -> String {
    let s = format!("{:.0}", n.round_dp(0));
    let (signo, dig) = s.strip_prefix('-').map_or(("", s.as_str()), |d| ("-", d));
    let mut out = String::new();
    for (i, c) in dig.chars().enumerate() {
        if i > 0 && (dig.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{signo}${out}")
}

fn pct(parte: Decimal, total: Decimal) -> u32 {
    if total.is_zero() { 0 } else { (parte / total * Decimal::from(100)).round().to_u32().unwrap_or(0) }
}

/// Una barra: etiqueta, monto, ancho relativo a la mayor (0-100) y porcentaje del total.
pub struct Barra {
    pub nombre: String,
    pub monto: String,
    pub ancho: u32,
    pub porcentaje: u32,
}

pub struct Comercio {
    pub nombre: String,
    pub categoria: String,
    pub total: String,
    pub cargos: usize,
    total_num: Decimal,
}

impl Comercio {
    pub fn es(&self, categoria: &str) -> bool {
        self.categoria == categoria
    }
}

/// Algo que se repite: suscripción o gasto hormiga.
pub struct Recurrente {
    pub nombre: String,
    pub cargos: usize,
    pub meses: usize,
    pub tipico: String,
    pub al_mes: String,
    pub al_anio: String,
}

pub struct Tablero {
    pub periodo: String,
    pub consumo: String,
    pub al_mes: String,
    pub entradas: String,
    pub fuera: Vec<(String, String)>,
    pub categorias: Vec<Barra>,
    pub meses: Vec<Barra>,
    pub hay_parcial: bool,
    pub suscripciones: Vec<Recurrente>,
    pub hormiga: Vec<Recurrente>,
    pub hormiga_total_mes: String,
    pub comercios: Vec<Comercio>,
    pub sin_categoria: usize,
}

const MESES_CORTOS: [&str; 12] = ["Ene", "Feb", "Mar", "Abr", "May", "Jun", "Jul", "Ago", "Sep", "Oct", "Nov", "Dic"];

fn barras(mut v: Vec<(String, Decimal)>, total: Decimal, ordenar: bool) -> Vec<Barra> {
    if ordenar {
        v.sort_by(|a, b| b.1.cmp(&a.1));
    }
    let max = v.iter().map(|x| x.1).max().unwrap_or_default();
    v.into_iter()
        .map(|(nombre, m)| Barra { nombre, monto: pesos(m), ancho: pct(m, max), porcentaje: pct(m, total) })
        .collect()
}

pub fn armar(movs: &[Mov], guardadas: &HashMap<String, String>) -> Option<Tablero> {
    let inicio = movs.iter().map(|m| m.fecha).min()?;
    let fin = movs.iter().map(|m| m.fecha).max()?;
    // Meses equivalentes del periodo cubierto (los estados van del 23 al 22, no por mes calendario).
    let dias = Decimal::from((fin - inicio).whole_days() + 1);
    let meses_periodo = (dias / Decimal::new(3044, 2)).max(Decimal::ONE);

    struct Acum {
        categoria: String,
        total: Decimal,
        montos: Vec<Decimal>,
        meses: BTreeSet<(i32, u8)>,
    }
    let mut por_comercio: BTreeMap<String, Acum> = BTreeMap::new();
    let mut por_categoria: HashMap<String, Decimal> = HashMap::new();
    let mut por_mes: BTreeMap<(i32, u8), Decimal> = BTreeMap::new();
    let (mut consumo, mut entradas) = (Decimal::ZERO, Decimal::ZERO);

    for m in movs {
        entradas += m.abono.unwrap_or_default();
        let Some(cargo) = m.cargo else { continue };
        let nombre = comercio(&m.descripcion);
        let categoria = categoria_de(&nombre, guardadas);
        let mes = (m.fecha.year(), m.fecha.month() as u8);
        let a = por_comercio.entry(nombre).or_insert_with(|| Acum {
            categoria: categoria.clone(),
            total: Decimal::ZERO,
            montos: Vec::new(),
            meses: BTreeSet::new(),
        });
        a.total += cargo;
        a.montos.push(cargo);
        a.meses.insert(mes);
        *por_categoria.entry(categoria.clone()).or_default() += cargo;
        if !NO_CONSUMO.contains(&categoria.as_str()) {
            consumo += cargo;
            *por_mes.entry(mes).or_default() += cargo;
        }
    }

    let fuera = NO_CONSUMO
        .iter()
        .filter_map(|c| por_categoria.get(*c).map(|t| (c.to_string(), pesos(*t))))
        .collect();
    let categorias = barras(
        por_categoria.into_iter().filter(|(c, _)| !NO_CONSUMO.contains(&c.as_str())).collect(),
        consumo,
        true,
    );
    // Primer y último mes casi siempre están incompletos (el estado empieza el 23).
    let hay_parcial = inicio.day() > 1 || fin.day() < fin.month().length(fin.year());
    let meses = barras(
        por_mes
            .into_iter()
            .map(|((a, m), t)| (format!("{} {}", MESES_CORTOS[m as usize - 1], a), t))
            .collect(),
        consumo,
        false,
    );

    let mut suscripciones = Vec::new();
    let mut hormiga = Vec::new();
    let mut hormiga_total = Decimal::ZERO;
    for (nombre, a) in &por_comercio {
        if NO_CONSUMO.contains(&a.categoria.as_str()) || a.categoria == "Efectivo" {
            continue;
        }
        let n = a.montos.len();
        let m = a.meses.len();
        let promedio = a.total / Decimal::from(n);
        let (min, max) = (a.montos.iter().min().copied().unwrap_or_default(), a.montos.iter().max().copied().unwrap_or_default());
        let al_mes = a.total / meses_periodo;
        let r = Recurrente {
            nombre: nombre.clone(),
            cargos: n,
            meses: m,
            tipico: pesos(promedio),
            al_mes: pesos(al_mes),
            al_anio: pesos(al_mes * Decimal::from(12)),
        };
        // Suscripción: aparece en 3+ meses, ~1 cargo al mes y siempre por un monto parecido.
        if m >= 3 && 2 * n <= 3 * m && max <= min * Decimal::new(15, 1) {
            suscripciones.push((a.total, r));
        // Hormiga: muchos cargos chicos.
        } else if n >= 8 && promedio <= Decimal::from(250) {
            hormiga_total += al_mes;
            hormiga.push((a.total, r));
        }
    }
    let ordenar = |mut v: Vec<(Decimal, Recurrente)>| {
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.into_iter().map(|x| x.1).collect::<Vec<_>>()
    };

    let mut comercios: Vec<Comercio> = por_comercio
        .into_iter()
        .map(|(nombre, a)| Comercio { nombre, categoria: a.categoria, total: pesos(a.total), cargos: a.montos.len(), total_num: a.total })
        .collect();
    comercios.sort_by(|a, b| b.total_num.cmp(&a.total_num));
    let sin_categoria = comercios.iter().filter(|c| c.categoria == "Otros" && !guardadas.contains_key(&c.nombre)).count();

    let f = |d: Date| format!("{:02}/{:02}/{}", d.day(), d.month() as u8, d.year());
    Some(Tablero {
        periodo: format!("{} al {}", f(inicio), f(fin)),
        consumo: pesos(consumo),
        al_mes: pesos(consumo / meses_periodo),
        entradas: pesos(entradas),
        fuera,
        categorias,
        meses,
        hay_parcial,
        suscripciones: ordenar(suscripciones),
        hormiga: ordenar(hormiga),
        hormiga_total_mes: pesos(hormiga_total),
        comercios,
        sin_categoria,
    })
}

// ---------- Clasificación con IA (Ollama) ----------

#[derive(Deserialize)]
struct Clasificadas {
    categorias: HashMap<String, String>,
}

pub async fn guardadas(st: &AppState, id: Uuid) -> Result<HashMap<String, String>, AppError> {
    let filas: Vec<(String, String)> =
        sqlx::query_as("SELECT comercio, categoria FROM categorias_comercio WHERE usuario_id = $1")
            .bind(id)
            .fetch_all(&st.db)
            .await?;
    Ok(filas.into_iter().collect())
}

/// Pide a la IA la categoría de los comercios que ninguna regla reconoce y la guarda.
/// No pisa lo que el usuario ya eligió. Devuelve cuántos clasificó.
pub async fn clasificar_pendientes(st: &AppState, id: Uuid) -> Result<usize, AppError> {
    let guardadas = guardadas(st, id).await?;
    let descripciones: Vec<String> =
        sqlx::query_scalar("SELECT DISTINCT descripcion FROM movimientos WHERE usuario_id = $1 AND cargo IS NOT NULL")
            .bind(id)
            .fetch_all(&st.db)
            .await?;
    let pendientes: Vec<String> = descripciones
        .iter()
        .map(|d| comercio(d))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|c| !guardadas.contains_key(c) && por_palabra(c).is_none())
        .collect();

    let lista = CATEGORIAS.join(", ");
    let mut hechos = 0;
    for grupo in pendientes.chunks(40) {
        let prompt = format!(
            "Eres un asistente de finanzas personales en México. Clasifica cada comercio de un estado \
             de cuenta en UNA de estas categorías: {lista}.\n\
             - \"MERPAGO*X\" es un pago con Mercado Pago al negocio X; clasifica según X.\n\
             - Si no sabes qué es, usa \"Otros\".\n\
             Devuelve SOLO JSON: {{\"categorias\": {{\"COMERCIO\": \"Categoría\"}}}} con los nombres tal cual.\n\n\
             Comercios:\n{}",
            grupo.join("\n"),
        );
        let respuesta = crate::banco::ollama_json(st, prompt).await?;
        let Ok(r) = serde_json::from_str::<Clasificadas>(&respuesta) else { continue };
        for c in grupo {
            let cat = r.categorias.get(c).filter(|x| CATEGORIAS.contains(&x.as_str())).map_or("Otros", |x| x.as_str());
            sqlx::query(
                "INSERT INTO categorias_comercio (usuario_id, comercio, categoria) VALUES ($1, $2, $3)
                 ON CONFLICT DO NOTHING",
            )
            .bind(id)
            .bind(c)
            .bind(cat)
            .execute(&st.db)
            .await?;
            hechos += 1;
        }
    }
    Ok(hechos)
}

// ---------- Handlers ----------

#[derive(Template, WebTemplate)]
#[template(path = "gastos.html")]
pub struct Pagina {
    t: Option<Tablero>,
    categorias: &'static [&'static str],
}

/// Tablero de gastos del usuario (None si no ha subido estados de cuenta).
pub async fn tablero(st: &AppState, id: Uuid) -> Result<Option<Tablero>, AppError> {
    let movs: Vec<Mov> = sqlx::query_as("SELECT fecha, descripcion, cargo, abono FROM movimientos WHERE usuario_id = $1")
        .bind(id)
        .fetch_all(&st.db)
        .await?;
    let guardadas = guardadas(st, id).await?;
    Ok(armar(&movs, &guardadas))
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Pagina, AppError> {
    Ok(Pagina { t: tablero(&st, id).await?, categorias: &CATEGORIAS })
}

#[derive(Deserialize)]
pub struct Cambio {
    comercio: String,
    categoria: String,
}

/// El usuario corrige la categoría de un comercio; aplica a todos sus cargos, pasados y futuros.
pub async fn cambiar(State(st): State<AppState>, Usuario(id): Usuario, Form(f): Form<Cambio>) -> Result<Redirect, AppError> {
    if !CATEGORIAS.contains(&f.categoria.as_str()) {
        return Err(AppError::usuario("Categoría inválida."));
    }
    sqlx::query(
        "INSERT INTO categorias_comercio (usuario_id, comercio, categoria) VALUES ($1, $2, $3)
         ON CONFLICT (usuario_id, comercio) DO UPDATE SET categoria = EXCLUDED.categoria",
    )
    .bind(id)
    .bind(&f.comercio)
    .bind(&f.categoria)
    .execute(&st.db)
    .await?;
    Ok(Redirect::to("/gastos#comercios"))
}

pub async fn clasificar(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Redirect, AppError> {
    clasificar_pendientes(&st, id).await?;
    Ok(Redirect::to("/gastos"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn nombre_de_comercio() {
        assert_eq!(comercio("AMAZON PRIME*JE3DQ1393"), "AMAZON PRIME");
        assert_eq!(comercio("DEPOP* P465842135109"), "DEPOP");
        assert_eq!(comercio("STEAMGAMES.COM 4259522985"), "STEAMGAMES.COM");
        assert_eq!(comercio("Order o-yhpnj1e"), "ORDER");
        assert_eq!(comercio("MERPAGO*TACOSDERES"), "MERPAGO*TACOSDERES");
        assert_eq!(comercio("DOMINO'S 7501"), "DOMINO'S");
        assert_eq!(comercio("7 ELEVEN LOMAS DE SOTE"), "7 ELEVEN LOMAS DE SOTE");
    }

    #[test]
    fn reglas() {
        assert_eq!(por_palabra("WAL MART LOMAS CASH"), Some("Efectivo"));
        assert_eq!(por_palabra("WAL MART LOMAS"), Some("Súper"));
        assert_eq!(por_palabra("AMAZON PRIME"), Some("Suscripciones"));
        assert_eq!(por_palabra("AMAZON COM"), Some("Compras en línea"));
        assert_eq!(por_palabra("UBER * EATS PENDING"), Some("Comida"));
        assert_eq!(por_palabra("UBR* PENDING.UBER.COM"), Some("Transporte"));
        assert_eq!(por_palabra("SPEI ENVIADO GBM"), Some("Inversión"));
        assert_eq!(por_palabra("SPEI ENVIADO BANAMEX"), Some("Transferencias"));
        assert_eq!(por_palabra("TCONE AGREGADOR"), None);
        assert!(REGLAS.iter().all(|(c, _)| CATEGORIAS.contains(c)));
    }

    #[test]
    fn tablero() {
        let d = |m: u8, dia: u8| Date::from_calendar_date(2026, time::Month::try_from(m).unwrap(), dia).unwrap();
        let cargo = |f: Date, desc: &str, c: &str| Mov { fecha: f, descripcion: desc.into(), cargo: Some(Decimal::from_str(c).unwrap()), abono: None };
        let mut movs = vec![
            Mov { fecha: d(3, 23), descripcion: "PAGO DE NOMINA".into(), cargo: None, abono: Some(Decimal::from(30000)) },
            cargo(d(3, 25), "SPEI ENVIADO GBM", "5000"), // inversión: no es consumo
        ];
        for (m, folio) in [(3, "A1"), (4, "B2"), (5, "C3")] {
            movs.push(cargo(d(m, 28), &format!("AMAZON PRIME*{folio}"), "99"));
        }
        for i in 0..9 {
            movs.push(cargo(d(4, 1 + i), "7 ELEVEN LOMAS", "50"));
        }
        let mut guardadas = HashMap::new();
        guardadas.insert("TACOS JUAN".to_string(), "Comida".to_string());
        movs.push(cargo(d(5, 22), "TACOS JUAN", "120"));

        let t = armar(&movs, &guardadas).unwrap();
        assert_eq!(t.consumo, "$867"); // 3×99 + 9×50 + 120, sin la inversión
        assert_eq!(t.entradas, "$30,000");
        assert_eq!(t.fuera, vec![("Inversión".to_string(), "$5,000".to_string())]);
        assert_eq!(t.suscripciones.len(), 1);
        assert_eq!(t.suscripciones[0].nombre, "AMAZON PRIME");
        assert_eq!(t.hormiga.len(), 1);
        assert_eq!(t.hormiga[0].nombre, "7 ELEVEN LOMAS");
        assert_eq!(t.categorias[0].nombre, "Tiendas de conveniencia");
        assert!(t.comercios.iter().any(|c| c.nombre == "TACOS JUAN" && c.categoria == "Comida"));
        assert!(t.hay_parcial);
    }

    // Revisión manual con los datos de la base local: cargo test real_gastos -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn real_gastos() {
        let db = sqlx::PgPool::connect(&crate::var("DATABASE_URL", "postgres://contaduria:contaduria@localhost/contaduria")).await.unwrap();
        let movs: Vec<Mov> = sqlx::query_as("SELECT fecha, descripcion, cargo, abono FROM movimientos").fetch_all(&db).await.unwrap();
        let filas: Vec<(String, String)> = sqlx::query_as("SELECT comercio, categoria FROM categorias_comercio").fetch_all(&db).await.unwrap();
        let t = armar(&movs, &filas.into_iter().collect()).expect("sin movimientos");
        println!("{} | gasto {} ({} al mes) | entradas {} | fuera {:?}", t.periodo, t.consumo, t.al_mes, t.entradas, t.fuera);
        for b in &t.categorias { println!("  {:<24} {:>10} {:>3}%", b.nombre, b.monto, b.porcentaje); }
        for b in &t.meses { println!("  {:<10} {:>10}", b.nombre, b.monto); }
        for s in &t.suscripciones { println!("  SUSC {:<28} {} x{} -> {}/año", s.nombre, s.tipico, s.cargos, s.al_anio); }
        for h in &t.hormiga { println!("  HORM {:<28} {} x{} -> {}/mes", h.nombre, h.tipico, h.cargos, h.al_mes); }
        println!("  sin categoría: {}", t.sin_categoria);
        for k in t.comercios.iter().filter(|k| k.categoria == "Otros") { println!("  OTROS {} {}", k.nombre, k.total); }
    }

    // Igual que el botón "Clasificar con IA", contra la base y el Ollama locales.
    #[tokio::test]
    #[ignore]
    async fn real_clasificar() {
        let db = sqlx::PgPool::connect(&crate::var("DATABASE_URL", "postgres://contaduria:contaduria@localhost/contaduria")).await.unwrap();
        let id: Uuid = sqlx::query_scalar("SELECT usuario_id FROM movimientos LIMIT 1").fetch_one(&db).await.unwrap();
        let origen = webauthn_rs::prelude::Url::parse("http://localhost:3000").unwrap();
        let st = AppState {
            db,
            webauthn: std::sync::Arc::new(webauthn_rs::prelude::WebauthnBuilder::new("localhost", &origen).unwrap().build().unwrap()),
            http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build().unwrap(),
        };
        let n = clasificar_pendientes(&st, id).await.map_err(|e| e.1).unwrap();
        let filas: Vec<(String, String)> = sqlx::query_as("SELECT comercio, categoria FROM categorias_comercio ORDER BY categoria")
            .fetch_all(&st.db).await.unwrap();
        println!("clasificados: {n}");
        for (c, cat) in filas { println!("  {cat:<24} {c}"); }
    }

    #[test]
    fn formato_pesos() {
        assert_eq!(pesos(Decimal::from_str("1234567.4").unwrap()), "$1,234,567");
        assert_eq!(pesos(Decimal::from(999)), "$999");
    }
}
