//! Estados de cuenta: subes un PDF, se leen los movimientos y se guardan.
//!
//! Para BBVA se usa un parser por columnas (exacto). Si no se reconoce el formato,
//! se recurre a un LLM local (Ollama) como respaldo.

use std::{process::Command, str::FromStr};

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::{Multipart, State}, response::{IntoResponse, Redirect, Response}};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Date, macros::format_description};
use tower_sessions::Session;
use uuid::Uuid;

use crate::{AppError, AppState, auth::Usuario};

/// Un movimiento ya normalizado, listo para guardar.
struct Movimiento {
    fecha: Date,
    descripcion: String,
    rfc: Option<String>,
    cargo: Option<Decimal>,
    abono: Option<Decimal>,
}

const MESES: [&str; 12] = ["ENE", "FEB", "MAR", "ABR", "MAY", "JUN", "JUL", "AGO", "SEP", "OCT", "NOV", "DIC"];

// --- Utilidades de texto ---

/// Corre pdftotext sobre los bytes del PDF. `modo`: "-layout" (conserva columnas, para bancos)
/// o "-raw" (orden del contenido, para GBM).
// ponytail: dependemos del binario pdftotext (poppler); una lib de Rust sería más código por nada.
pub fn texto_pdf(pdf: &[u8], modo: &str) -> Result<String, AppError> {
    let ruta = std::env::temp_dir().join(format!("contaduria-{}.pdf", uuid::Uuid::new_v4()));
    std::fs::write(&ruta, pdf)?;
    let salida = Command::new("pdftotext").arg(modo).arg(&ruta).arg("-").output();
    let _ = std::fs::remove_file(&ruta);
    let salida = salida.map_err(|_| AppError::usuario("No se pudo leer el PDF (falta pdftotext)."))?;
    if !salida.status.success() {
        return Err(AppError::usuario("El PDF no se pudo leer."));
    }
    Ok(String::from_utf8_lossy(&salida.stdout).into_owned())
}

/// Primera fecha dd/mm/aaaa del texto → (mes, año). El periodo trae dos: inicio y fin.
fn fechas_periodo(texto: &str) -> Vec<(u8, i32)> {
    let b = texto.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i + 10 <= b.len() {
        // patrón dd/dd/dddd
        let s = &b[i..i + 10];
        if s[0].is_ascii_digit() && s[1].is_ascii_digit() && s[2] == b'/'
            && s[3].is_ascii_digit() && s[4].is_ascii_digit() && s[5] == b'/'
            && s[6..10].iter().all(u8::is_ascii_digit)
        {
            let mes: u8 = std::str::from_utf8(&s[3..5]).unwrap().parse().unwrap_or(0);
            let anio: i32 = std::str::from_utf8(&s[6..10]).unwrap().parse().unwrap_or(0);
            v.push((mes, anio));
            if v.len() == 2 {
                break;
            }
            i += 10;
        } else {
            i += 1;
        }
    }
    v
}

/// Año que corresponde a un mes, según el periodo (maneja el cruce diciembre→enero).
fn anio_de(mes: u8, periodo: &[(u8, i32)]) -> Option<i32> {
    match periodo {
        [(_, a)] => Some(*a),
        [(m0, a0), (_, a1)] if a0 == a1 => Some(*a0),
        [(m0, a0), (_, a1)] => Some(if mes >= *m0 { *a0 } else { *a1 }),
        _ => None,
    }
}

/// ¿Es un token un monto en pesos? (trae punto decimal y dos decimales; así se descartan
/// los números de la descripción y las referencias, que no llevan punto.)
fn es_monto(t: &[char]) -> bool {
    let s: String = t.iter().collect();
    let Some((ent, dec)) = s.rsplit_once('.') else { return false };
    dec.len() == 2
        && dec.chars().all(|c| c.is_ascii_digit())
        && !ent.is_empty()
        && ent.chars().all(|c| c.is_ascii_digit() || c == ',')
}

fn a_decimal(t: &[char]) -> Option<Decimal> {
    let limpio: String = t.iter().filter(|c| **c != ',').collect();
    Decimal::from_str(&limpio).ok()
}

/// Tokens no vacíos de una línea, con su columna (índice de carácter) inicial y final.
fn tokens(chars: &[char]) -> Vec<(usize, usize, &[char])> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' {
            i += 1;
            continue;
        }
        let s = i;
        while i < chars.len() && chars[i] != ' ' {
            i += 1;
        }
        v.push((s, i, &chars[s..i])); // fin exclusivo
    }
    v
}

/// ¿El token es una fecha "23/SEP"? Devuelve (día, mes 1-12).
fn fecha_corta(t: &[char]) -> Option<(u8, u8)> {
    let s: String = t.iter().collect();
    let (dia, mes) = s.split_once('/')?;
    let dia: u8 = dia.parse().ok()?;
    let mes = MESES.iter().position(|m| m.eq_ignore_ascii_case(mes))? as u8 + 1;
    Some((dia, mes))
}

/// Busca un RFC (12-13) en un texto, aunque venga partido por espacios ("NWM 9709244W4").
fn buscar_rfc(texto: &str) -> Option<String> {
    let i = texto.find("RFC:")? + 4;
    let mut cand = String::new();
    for tok in texto[i..].split_whitespace() {
        if !tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '&' || c == 'Ñ') {
            break; // algo como "09:02" o "AUT:" corta el RFC
        }
        cand.push_str(tok);
        if rfc_valido(&cand) {
            return Some(cand.to_uppercase());
        }
        if cand.len() > 13 {
            break;
        }
    }
    None
}

/// RFC de persona moral (12) o física (13): letras, 6 dígitos de fecha, 3 de homoclave.
fn rfc_valido(s: &str) -> bool {
    let n = s.len();
    if n != 12 && n != 13 {
        return false;
    }
    let letras = n - 9;
    let b: Vec<char> = s.chars().collect();
    b[..letras].iter().all(|c| c.is_ascii_alphabetic() || *c == '&' || *c == 'Ñ')
        && b[letras..letras + 6].iter().all(|c| c.is_ascii_digit())
        && b[letras + 6..].iter().all(|c| c.is_ascii_alphanumeric())
}

// --- Parser BBVA por columnas ---

/// Intenta leer un estado de cuenta BBVA. None si no reconoce el formato (→ usar LLM).
fn parsear_bbva(texto: &str) -> Option<Vec<Movimiento>> {
    let lineas: Vec<&str> = texto.lines().collect();
    // El encabezado de la tabla tiene CARGOS y ABONOS.
    let h = lineas.iter().position(|l| l.contains("CARGOS") && l.contains("ABONOS"))?;
    let periodo = fechas_periodo(texto);

    // Agrupa cada movimiento: la línea que empieza con fecha, más sus líneas de detalle.
    struct Bloque {
        chars: Vec<char>,   // la línea de la fecha
        detalle: String,    // esa línea + continuaciones (para buscar el RFC)
        dia: u8,
        mes: u8,
    }
    let mut bloques: Vec<Bloque> = Vec::new();
    for l in &lineas[h + 1..] {
        let chars: Vec<char> = l.chars().collect();
        let toks = tokens(&chars);
        if let Some((dia, mes)) = toks.first().and_then(|(_, _, t)| fecha_corta(t)) {
            bloques.push(Bloque { chars: chars.clone(), detalle: l.to_string(), dia, mes });
        } else if let Some(b) = bloques.last_mut() {
            b.detalle.push(' ');
            b.detalle.push_str(l);
        }
    }
    if bloques.is_empty() {
        return None;
    }

    // Primer monto de cada línea = el movimiento; los demás son saldos. Guardamos su columna
    // para luego decidir, por posición, si es cargo o abono.
    struct Crudo<'a> {
        bloque: &'a Bloque,
        monto: Decimal,
        col: usize,
    }
    let mut crudos: Vec<Crudo> = Vec::new();
    for b in &bloques {
        for (_, fin, t) in tokens(&b.chars) {
            if es_monto(t) {
                if let Some(m) = a_decimal(t) {
                    crudos.push(Crudo { bloque: b, monto: m, col: fin });
                }
                break; // solo el primer monto
            }
        }
    }
    if crudos.is_empty() {
        return None;
    }

    // Las columnas de cargo y abono salen de los datos: los montos se alinean a la derecha,
    // así que sus posiciones finales forman grupos. El grupo de la izquierda es CARGO; el de
    // la derecha, ABONO (orden del encabezado de BBVA).
    let mut cols: Vec<usize> = crudos.iter().map(|c| c.col).collect();
    cols.sort_unstable();
    cols.dedup();
    let corte = cols.windows(2).find(|w| w[1] - w[0] > 6).map(|w| w[1]);
    let (col_cargo, col_abono) = match corte {
        Some(c) => {
            let izq: Vec<usize> = cols.iter().copied().filter(|&x| x < c).collect();
            let der: Vec<usize> = cols.iter().copied().filter(|&x| x >= c).collect();
            (promedio(&izq), Some(promedio(&der)))
        }
        // ponytail: un solo grupo = todos del mismo tipo; se asume cargo (los gastos son lo
        // común). Mejorar leyendo la posición del encabezado si aparecen estados solo de abonos.
        None => (promedio(&cols), None),
    };

    let mut movs = Vec::new();
    for c in crudos {
        let anio = anio_de(c.bloque.mes, &periodo)?;
        let fecha = Date::from_calendar_date(anio, mes_time(c.bloque.mes)?, c.bloque.dia).ok()?;
        let es_abono = col_abono.is_some_and(|ca| dist(c.col, ca) < dist(c.col, col_cargo));
        movs.push(Movimiento {
            fecha,
            descripcion: descripcion(&c.bloque.chars),
            rfc: buscar_rfc(&c.bloque.detalle),
            cargo: (!es_abono).then_some(c.monto),
            abono: es_abono.then_some(c.monto),
        });
    }
    Some(movs)
}

fn promedio(v: &[usize]) -> usize {
    if v.is_empty() { 0 } else { v.iter().sum::<usize>() / v.len() }
}
fn dist(a: usize, b: usize) -> usize {
    a.abs_diff(b)
}
fn mes_time(mes: u8) -> Option<time::Month> {
    time::Month::try_from(mes).ok()
}

/// Descripción: lo que hay entre las dos fechas y el primer monto.
fn descripcion(chars: &[char]) -> String {
    let toks = tokens(chars);
    let inicio = toks.iter().take_while(|(_, _, t)| fecha_corta(t).is_some()).count();
    let mut palabras = Vec::new();
    for (_, _, t) in &toks[inicio..] {
        if es_monto(t) {
            break;
        }
        palabras.push(t.iter().collect::<String>());
    }
    palabras.join(" ")
}

// --- Respaldo con LLM local (Ollama) ---

#[derive(Deserialize)]
struct Extraido {
    movimientos: Vec<MovimientoLeido>,
}
#[derive(Deserialize)]
struct MovimientoLeido {
    fecha: String,
    descripcion: String,
    rfc: Option<String>,
    cargo: Option<String>,
    abono: Option<String>,
}
#[derive(Serialize)]
struct PedidoOllama<'a> {
    model: &'a str,
    prompt: String,
    stream: bool,
    // Sin `format: "json"`: gemma4 en Ollama 0.40 responde "structured output is unavailable".
    think: bool,
}
#[derive(Deserialize)]
struct RespuestaOllama {
    response: String,
}

fn opcional(s: Option<String>) -> Option<String> {
    s.map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && t.to_lowercase() != "null")
}
fn monto_str(s: Option<String>) -> Option<Decimal> {
    let s = opcional(s)?;
    let limpio: String = s.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    Decimal::from_str(&limpio).ok()
}

/// Manda un prompt a Ollama y regresa su respuesta (JSON en texto).
pub async fn ollama_json(st: &AppState, prompt: String) -> Result<String, AppError> {
    let url = crate::var("OLLAMA_URL", "http://localhost:11434");
    let modelo = crate::var("OLLAMA_MODEL", "gemma4");
    let r = st
        .http
        .post(format!("{url}/api/generate"))
        .json(&PedidoOllama { model: &modelo, prompt, stream: false, think: false })
        .send()
        .await
        .map_err(|_| AppError::usuario("No se pudo contactar a Ollama. ¿Está corriendo?"))?;
    let cuerpo: RespuestaOllama = r.json().await?;
    Ok(solo_json(&cuerpo.response).to_string())
}

/// El objeto JSON dentro de la respuesta del modelo (suele venir entre ```json ... ```).
fn solo_json(s: &str) -> &str {
    match (s.find('{'), s.rfind('}')) {
        (Some(i), Some(f)) if i < f => &s[i..=f],
        _ => s,
    }
}

async fn extraer_llm(st: &AppState, texto: &str) -> Result<Vec<Movimiento>, AppError> {
    let periodo_txt = texto.find("DEL ").map(|i| texto[i..].chars().take(32).collect::<String>()).unwrap_or_default();
    let region = texto.find("Detalle de Movimientos").map(|i| &texto[i..]).unwrap_or(texto);
    let prompt = format!(
        "Extrae los movimientos de este estado de cuenta.\n\
         Periodo: {periodo_txt}. Fechas como \"23/SEP\" → AAAA-MM-DD con el año del periodo.\n\
         Devuelve SOLO JSON: {{\"movimientos\":[{{\"fecha\":\"AAAA-MM-DD\",\"descripcion\":\"...\",\"rfc\":\"RFC o null\",\"cargo\":\"monto o null\",\"abono\":\"monto o null\"}}]}}\n\
         - Los últimos números de cada línea son SALDOS: ignóralos. cada movimiento tiene cargo O abono.\n\
         - cargo = dinero que salió; abono = dinero que entró. rfc sin espacios o null.\n\n\
         TEXTO:\n{region}",
    );
    let respuesta = ollama_json(st, prompt).await?;
    let extraido: Extraido = serde_json::from_str(&respuesta)
        .map_err(|_| AppError::usuario("El modelo no devolvió un JSON válido. Intenta de nuevo."))?;
    let iso = format_description!("[year]-[month]-[day]");
    Ok(extraido
        .movimientos
        .into_iter()
        .filter_map(|m| {
            Some(Movimiento {
                fecha: Date::parse(&m.fecha, iso).ok()?,
                descripcion: m.descripcion.trim().to_string(),
                rfc: opcional(m.rfc),
                cargo: monto_str(m.cargo),
                abono: monto_str(m.abono),
            })
        })
        .collect())
}

// --- Conciliación: casar movimientos del banco con CFDI por RFC, monto y fecha ---

/// Marca cada movimiento sin conciliar que tenga un CFDI del mismo RFC, mismo monto y
/// fecha a ±5 días. Devuelve cuántos se conciliaron.
// ponytail: emparejado simple; si hay dos cargos idénticos al mismo RFC pueden apuntar al
// mismo CFDI. Un emparejado 1-a-1 estricto se agrega si aparece el caso.
pub async fn conciliar(db: &PgPool, id: Uuid) -> Result<u64, AppError> {
    // Pasada 1 (fuerte): mismo RFC (como emisor o receptor del CFDI), mismo monto, fecha ±5 días.
    // Sirve para compras con tarjeta, donde el estado de cuenta sí trae "RFC:".
    let por_rfc = sqlx::query(
        "UPDATE movimientos m SET cfdi_uuid = c.uuid
         FROM cfdis c
         WHERE m.usuario_id = $1 AND m.cfdi_uuid IS NULL AND m.rfc IS NOT NULL
           AND c.usuario_id = $1 AND c.total = COALESCE(m.cargo, m.abono)
           AND (c.emisor_rfc = m.rfc OR c.receptor_rfc = m.rfc)
           AND ABS(c.fecha::date - m.fecha) <= 5",
    )
    .bind(id)
    .execute(db)
    .await?
    .rows_affected();

    // Pasada 2: por monto y fecha ±3 días, solo cuando hay UN único CFDI candidato (evita
    // emparejar al azar). Caza nómina e inversiones, que llegan por SPEI sin RFC en el banco.
    // ponytail: no garantiza que un CFDI se use una sola vez; basta mientras los montos sean
    // distintivos. Emparejado 1-a-1 estricto si aparece el caso.
    let por_monto = sqlx::query(
        "UPDATE movimientos m SET cfdi_uuid = sub.uuid
         FROM (
            SELECT m.id AS mid, min(c.uuid::text) AS uuid
            FROM movimientos m
            JOIN cfdis c ON c.usuario_id = m.usuario_id
                        AND c.total = COALESCE(m.cargo, m.abono)
                        AND c.total > 0
                        AND ABS(c.fecha::date - m.fecha) <= 3
            WHERE m.usuario_id = $1 AND m.cfdi_uuid IS NULL
            GROUP BY m.id
            HAVING count(*) = 1
         ) sub
         WHERE m.id = sub.mid",
    )
    .bind(id)
    .execute(db)
    .await?
    .rows_affected();

    Ok(por_rfc + por_monto)
}

pub async fn conciliar_handler(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    conciliar(&st.db, id).await?;
    Ok(Redirect::to("/banco").into_response())
}

// --- Handlers ---

/// Texto canónico de los movimientos de un estado, para detectar re-subidas del mismo estado.
fn huella_movimientos(movs: &[Movimiento]) -> String {
    let mut lineas: Vec<String> = movs
        .iter()
        .map(|m| format!("{}|{}|{:?}|{:?}", m.fecha, m.descripcion, m.cargo.map(|d| d.normalize()), m.abono.map(|d| d.normalize())))
        .collect();
    lineas.sort();
    lineas.join("\n")
}

/// Lee el texto de un estado bancario y guarda sus movimientos. Devuelve cuántos guardó.
async fn guardar_pdf(st: &AppState, id: Uuid, archivo: &str, texto: &str) -> Result<usize, AppError> {
    let movs = match parsear_bbva(texto) {
        Some(m) if !m.is_empty() => m,
        _ => extraer_llm(st, texto).await?,
    };
    if movs.is_empty() {
        return Err(AppError::usuario("no se encontraron movimientos"));
    }

    // Re-subir el mismo estado reemplaza sus movimientos. La huella sale de los movimientos
    // leídos, no de los bytes del PDF: el banco genera un PDF distinto en cada descarga.
    // ponytail: con el respaldo LLM la lectura puede variar entre subidas y no detectar el
    // duplicado; huella por (cuenta, periodo) si eso pasa.
    let mut tx = st.db.begin().await?;
    let (huella,): (String,) = sqlx::query_as("SELECT encode(sha256(convert_to($1, 'UTF8')), 'hex')")
        .bind(huella_movimientos(&movs))
        .fetch_one(&mut *tx)
        .await?;
    // Filas de antes de 0006 no tienen huella: para ellas se conserva el reemplazo por nombre.
    sqlx::query(
        "DELETE FROM movimientos WHERE usuario_id = $1
           AND (huella = $2 OR (huella IS NULL AND archivo = $3))",
    )
        .bind(id)
        .bind(&huella)
        .bind(archivo)
        .execute(&mut *tx)
        .await?;
    for m in &movs {
        sqlx::query(
            "INSERT INTO movimientos (usuario_id, fecha, descripcion, rfc, cargo, abono, archivo, huella)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(id)
        .bind(m.fecha)
        .bind(&m.descripcion)
        .bind(&m.rfc)
        .bind(m.cargo)
        .bind(m.abono)
        .bind(archivo)
        .bind(&huella)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(movs.len())
}

/// Sube uno o varios PDFs. Un archivo que falla no detiene a los demás; el resultado se
/// muestra como aviso en /banco.
// ponytail: se leen en serie dentro del request; con muchos PDFs de otros bancos (Ollama)
// puede tardar minutos. Pasar a tareas.rs en segundo plano si se vuelve molesto.
pub async fn subir(
    State(st): State<AppState>,
    Usuario(id): Usuario,
    sesion: Session,
    mut mp: Multipart,
) -> Result<Response, AppError> {
    let mut leidos = 0;
    let mut movimientos = 0;
    let mut inversiones: Vec<String> = Vec::new();
    let mut errores = Vec::new();
    while let Some(campo) = mp
        .next_field()
        .await
        .map_err(|_| AppError::usuario("Archivo inválido o demasiado grande (máx. 50 MB en total)."))?
    {
        if campo.name() != Some("pdf") {
            continue;
        }
        let archivo = campo.file_name().unwrap_or("estado.pdf").to_string();
        let pdf = campo.bytes().await.map_err(|_| AppError::usuario("No se pudo leer el archivo."))?;
        if pdf.is_empty() {
            continue; // input vacío
        }
        // Los estados de inversión se reconocen y van a inversiones; el resto, a movimientos del banco.
        let resultado = match texto_pdf(&pdf, "-layout") {
            Ok(t) if crate::inversiones::es_inversion(&t) => {
                crate::inversiones::guardar(&st, id, &archivo, &pdf).await.map(|e| inversiones.push(e))
            }
            Ok(t) => guardar_pdf(&st, id, &archivo, &t).await.map(|n| {
                movimientos += n;
                eprintln!("banco: {n} movimientos de {archivo}");
            }),
            Err(e) => Err(e),
        };
        match resultado {
            Ok(()) => leidos += 1,
            Err(AppError(_, msg)) => errores.push(format!("{archivo}: {msg}")),
        }
    }
    if leidos == 0 && errores.is_empty() {
        return Err(AppError::usuario("Sube al menos un PDF."));
    }
    if movimientos > 0 {
        conciliar(&st.db, id).await?;
        // Si Ollama no está, los comercios nuevos quedan en "Otros"; no se pierde la subida.
        if let Err(AppError(_, e)) = crate::gastos::clasificar_pendientes(&st, id).await {
            eprintln!("gastos: no se clasificó con IA: {e}");
        }
    }

    let mut aviso = format!("{leidos} archivo(s) leído(s), {movimientos} movimientos.");
    if !inversiones.is_empty() {
        aviso.push_str(&format!(" Estados de inversión: {}.", inversiones.join(", ")));
    }
    if !errores.is_empty() {
        aviso.push_str(&format!(" No se pudieron leer: {}", errores.join("; ")));
    }
    sesion.insert("aviso_banco", aviso).await?;
    Ok(Redirect::to("/banco").into_response())
}

#[derive(sqlx::FromRow)]
pub struct Fila {
    fecha: Date,
    descripcion: String,
    rfc: Option<String>,
    cargo: Option<Decimal>,
    abono: Option<Decimal>,
    conciliado: bool,
}

impl Fila {
    pub fn fecha_texto(&self) -> String {
        self.fecha.format(format_description!("[day]/[month]/[year]")).unwrap_or_default()
    }
    pub fn rfc_texto(&self) -> &str {
        self.rfc.as_deref().unwrap_or("—")
    }
    pub fn cargo_texto(&self) -> String {
        self.cargo.map(|d| format!("${:.2}", d.round_dp(2))).unwrap_or_default()
    }
    pub fn abono_texto(&self) -> String {
        self.abono.map(|d| format!("${:.2}", d.round_dp(2))).unwrap_or_default()
    }
    pub fn conciliado(&self) -> bool {
        self.conciliado
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "banco.html")]
pub struct Pagina {
    filas: Vec<Fila>,
    total: i64,
    conciliados: i64,
    aviso: Option<String>,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario, sesion: Session) -> Result<Pagina, AppError> {
    let aviso: Option<String> = sesion.remove("aviso_banco").await?;
    let filas = sqlx::query_as(
        "SELECT fecha, descripcion, rfc, cargo, abono, (cfdi_uuid IS NOT NULL) AS conciliado
         FROM movimientos WHERE usuario_id = $1 ORDER BY fecha DESC, id DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    let (total, conciliados): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(cfdi_uuid) FROM movimientos WHERE usuario_id = $1",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(Pagina { filas, total, conciliados, aviso })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poner(buf: &mut Vec<char>, inicio0: usize, s: &str) {
        for (k, c) in s.chars().enumerate() {
            while buf.len() <= inicio0 + k {
                buf.push(' ');
            }
            buf[inicio0 + k] = c;
        }
    }
    // Coloca un texto que TERMINA en la columna fin (1-based), como los montos a la derecha.
    fn derecha(buf: &mut Vec<char>, fin: usize, s: &str) {
        poner(buf, fin - s.chars().count(), s);
    }
    fn linea(construir: impl Fn(&mut Vec<char>)) -> String {
        let mut b = Vec::new();
        construir(&mut b);
        b.into_iter().collect()
    }

    // Reproduce las columnas reales de un estado BBVA (CARGOS~119, ABONOS~131, saldos ~143 y ~160).
    #[test]
    fn parser_bbva_distingue_cargo_de_abono() {
        let mut texto = String::new();
        texto.push_str("Periodo   DEL 23/09/2026 AL 22/10/2026\n");
        texto.push_str(&linea(|b| {
            poner(b, 2, "OPER");
            poner(b, 27, "DESCRIPCION");
            poner(b, 102, "CARGOS");
            poner(b, 123, "ABONOS");
            poner(b, 130, "OPERACION");
            poner(b, 142, "LIQUIDACION");
        }));
        texto.push('\n');
        // Gasto: WAL MART, cargo 599.00 (fin 119) + dos saldos. Descripción con un "100" que NO es monto.
        texto.push_str(&linea(|b| {
            poner(b, 2, "25/SEP");
            poner(b, 14, "25/SEP");
            poner(b, 28, "WAL MART 100");
            derecha(b, 119, "599.00");
            derecha(b, 143, "4,492.40");
            derecha(b, 163, "4,326.40");
        }));
        texto.push('\n');
        texto.push_str(&linea(|b| poner(b, 28, "RFC: NWM 9709244W4 09:02 AUT: 1")));
        texto.push('\n');
        // Depósito: abono 2,400.00 (fin 131) + dos saldos.
        texto.push_str(&linea(|b| {
            poner(b, 2, "27/SEP");
            poner(b, 14, "28/SEP");
            poner(b, 28, "PAGO CUENTA DE TERCERO");
            derecha(b, 131, "2,400.00");
            derecha(b, 143, "6,355.31");
            derecha(b, 160, "427.39");
        }));
        texto.push('\n');

        let movs = parsear_bbva(&texto).expect("debe parsear");
        assert_eq!(movs.len(), 2);

        let gasto = &movs[0];
        assert_eq!(gasto.cargo, Some(Decimal::from_str("599.00").unwrap()));
        assert_eq!(gasto.abono, None, "el saldo 4,492.40 no debe tomarse como abono");
        assert_eq!(gasto.rfc.as_deref(), Some("NWM9709244W4"));
        assert_eq!(gasto.descripcion, "WAL MART 100");
        assert_eq!(gasto.fecha, Date::from_calendar_date(2026, time::Month::September, 25).unwrap());

        let dep = &movs[1];
        assert_eq!(dep.abono, Some(Decimal::from_str("2400.00").unwrap()), "2,400 es depósito, no cargo");
        assert_eq!(dep.cargo, None);
    }

    // Validación manual contra un PDF real: BANCO_PDF=/ruta cargo test real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real() {
        let ruta = std::env::var("BANCO_PDF").expect("define BANCO_PDF");
        let pdf = std::fs::read(&ruta).unwrap();
        let texto = texto_pdf(&pdf, "-layout").ok().unwrap();
        let movs = parsear_bbva(&texto).expect("no parseó");
        let cargos: Decimal = movs.iter().filter_map(|m| m.cargo).sum();
        let abonos: Decimal = movs.iter().filter_map(|m| m.abono).sum();
        println!("movimientos: {}  cargos: {cargos}  abonos: {abonos}", movs.len());
        for m in movs.iter().take(8) {
            println!("  {} | {:<28} | c {:?} a {:?} | {:?}", m.fecha, m.descripcion, m.cargo, m.abono, m.rfc);
        }
    }

    #[test]
    fn huella_ignora_orden_y_formato_del_monto() {
        let m = |d: u8, c: &str| Movimiento {
            fecha: Date::from_calendar_date(2026, time::Month::May, d).unwrap(),
            descripcion: "OXXO".into(),
            rfc: None,
            cargo: Some(Decimal::from_str(c).unwrap()),
            abono: None,
        };
        let a = huella_movimientos(&[m(1, "10.50"), m(2, "20.00")]);
        let b = huella_movimientos(&[m(2, "20"), m(1, "10.5")]);
        assert_eq!(a, b);
        assert_ne!(a, huella_movimientos(&[m(1, "10.50")]));
    }

    #[test]
    fn json_dentro_de_la_respuesta() {
        assert_eq!(solo_json("```json\n{\"a\": {\"b\": 1}}\n```"), "{\"a\": {\"b\": 1}}");
        assert_eq!(solo_json("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(solo_json("nada"), "nada");
    }

    #[test]
    fn rfc_y_montos() {
        assert!(rfc_valido("GATI820522K43")); // física 13
        assert!(rfc_valido("NWM9709244W4")); // moral 12
        assert!(!rfc_valido("17:47"));
        assert_eq!(buscar_rfc("RFC: GATI820522K43 20:28 AUT: 1").as_deref(), Some("GATI820522K43"));
        assert_eq!(buscar_rfc("RFC: 17:47 AUT: 2"), None);
        assert!(es_monto(&"2,400.00".chars().collect::<Vec<_>>()));
        assert!(!es_monto(&"100".chars().collect::<Vec<_>>()));
        assert!(!es_monto(&"09:02".chars().collect::<Vec<_>>()));
    }
}
