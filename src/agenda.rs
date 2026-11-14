//! Calendario (/calendario): en qué días gastaste (de tus estados de cuenta) y tus fechas
//! fiscales del mes. La fecha es la que reporta el banco, no necesariamente la de la compra.

use std::collections::HashMap;

use askama::Template;
use askama_web::WebTemplate;
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use time::{Date, Duration, Month};

use crate::{
    AppError, AppState,
    auth::Usuario,
    calendario::{Obligacion, obligaciones},
    gastos::{self, NO_CONSUMO, pesos},
};

const MESES: [&str; 12] = [
    "Enero", "Febrero", "Marzo", "Abril", "Mayo", "Junio",
    "Julio", "Agosto", "Septiembre", "Octubre", "Noviembre", "Diciembre",
];

/// "$350", "$1.2k", "$5k" o "$11k": cabe en la celda de un día en el celular.
fn compacto(n: Decimal) -> String {
    let n = n.round().to_i64().unwrap_or(0);
    match n {
        0..1000 => format!("${n}"),
        1000..10_000 => format!("${}k", format!("{:.1}", n as f64 / 1000.0).trim_end_matches(".0")),
        _ => format!("${}k", (n as f64 / 1000.0).round()),
    }
}

pub struct Dia {
    pub numero: u8,
    pub gasto: String,
    pub gasto_largo: String,
    pub compras: usize,
    /// Ancho de la barrita (0-100), relativo al día de más gasto del mes.
    pub nivel: u32,
    pub obligaciones: Vec<String>,
    pub hoy: bool,
    pub elegido: bool,
}

/// Semanas de lunes a domingo; None = celda de relleno fuera del mes.
fn semanas(
    inicio: Date,
    gasto: &HashMap<Date, (Decimal, usize)>,
    obligs: &[(Date, String)],
    hoy: Date,
    elegido: Option<Date>,
) -> Vec<Vec<Option<Dia>>> {
    let max = gasto.values().map(|g| g.0).max().unwrap_or_default();
    let mut celdas: Vec<Option<Dia>> = (0..inicio.weekday().number_days_from_monday()).map(|_| None).collect();
    let mut d = inicio;
    while d.month() == inicio.month() {
        let (g, n) = gasto.get(&d).copied().unwrap_or_default();
        celdas.push(Some(Dia {
            numero: d.day(),
            gasto: if n > 0 { compacto(g) } else { String::new() },
            gasto_largo: pesos(g),
            compras: n,
            nivel: if max.is_zero() { 0 } else { (g / max * Decimal::from(100)).round().to_u32().unwrap_or(0) },
            obligaciones: obligs.iter().filter(|o| o.0 == d).map(|o| o.1.clone()).collect(),
            hoy: d == hoy,
            elegido: elegido == Some(d),
        }));
        d += Duration::days(1);
    }
    while celdas.len() % 7 != 0 {
        celdas.push(None);
    }
    let mut celdas = celdas.into_iter();
    std::iter::from_fn(|| Some(celdas.by_ref().take(7).collect::<Vec<_>>()).filter(|s| !s.is_empty())).collect()
}

pub struct Compra {
    pub descripcion: String,
    pub categoria: String,
    pub cargo: String,
    pub abono: String,
    pub cuenta: bool,
}

#[derive(Template, WebTemplate)]
#[template(path = "agenda.html")]
pub struct Pagina {
    titulo: String,
    mes: String,
    anterior: String,
    siguiente: String,
    ultimo_con_datos: Option<(String, String)>,
    semanas: Vec<Vec<Option<Dia>>>,
    total: String,
    compras: usize,
    dia_titulo: String,
    dia_compras: Vec<Compra>,
    obligaciones: Vec<Obligacion>,
}

#[derive(Deserialize)]
pub struct Params {
    mes: Option<String>,
    dia: Option<u8>,
}

fn clave(d: Date) -> String {
    format!("{}-{:02}", d.year(), d.month() as u8)
}

fn primer_dia(clave: &str) -> Option<Date> {
    let (a, m) = clave.split_once('-')?;
    Date::from_calendar_date(a.parse().ok()?, Month::try_from(m.parse::<u8>().ok()?).ok()?, 1).ok()
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario, Query(q): Query<Params>) -> Result<Response, AppError> {
    let perfil: Option<(Vec<String>, Option<Date>)> =
        sqlx::query_as("SELECT regimenes, efirma_vence FROM perfiles WHERE usuario_id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    let Some((regimenes, efirma_vence)) = perfil else {
        return Ok(Redirect::to("/perfil").into_response());
    };
    let hoy = crate::ahora_mx().date();
    let inicio = q.mes.as_deref().and_then(primer_dia).unwrap_or_else(|| hoy.replace_day(1).unwrap());
    let fin = inicio + Duration::days(inicio.month().length(inicio.year()) as i64);

    let movs: Vec<(Date, String, Option<Decimal>, Option<Decimal>)> = sqlx::query_as(
        "SELECT fecha, descripcion, cargo, abono FROM movimientos
         WHERE usuario_id = $1 AND fecha >= $2 AND fecha < $3 ORDER BY fecha, id",
    )
    .bind(id)
    .bind(inicio)
    .bind(fin)
    .fetch_all(&st.db)
    .await?;
    let guardadas = gastos::guardadas(&st, id).await?;

    // Gasto de consumo por día (sin transferencias ni inversión, igual que en Gastos).
    let mut por_dia: HashMap<Date, (Decimal, usize)> = HashMap::new();
    let mut total = Decimal::ZERO;
    let elegido = q.dia.and_then(|d| inicio.replace_day(d).ok());
    let mut dia_compras = Vec::new();
    for (fecha, desc, cargo, abono) in &movs {
        let categoria = gastos::categoria(desc, &guardadas);
        let cuenta = cargo.is_some() && !NO_CONSUMO.contains(&categoria.as_str());
        if cuenta {
            let e = por_dia.entry(*fecha).or_default();
            e.0 += cargo.unwrap_or_default();
            e.1 += 1;
            total += cargo.unwrap_or_default();
        }
        if Some(*fecha) == elegido {
            dia_compras.push(Compra {
                descripcion: desc.clone(),
                categoria,
                cargo: cargo.map(pesos).unwrap_or_default(),
                abono: abono.map(pesos).unwrap_or_default(),
                cuenta,
            });
        }
    }

    // Fechas fiscales de este mes (sólo de hoy en adelante: el calendario fiscal no guarda historial).
    let obligs: Vec<Obligacion> = obligaciones(&regimenes, efirma_vence, hoy, 400)
        .into_iter()
        .filter(|o| o.fecha >= inicio && o.fecha < fin)
        .collect();
    let marcas: Vec<(Date, String)> = obligs.iter().map(|o| (o.fecha, o.titulo.clone())).collect();

    // Si este mes no tiene movimientos, ofrece ir al último que sí tenga.
    let ultimo: Option<Date> = sqlx::query_scalar("SELECT max(fecha) FROM movimientos WHERE usuario_id = $1")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    let ultimo_con_datos = ultimo
        .filter(|_| movs.is_empty())
        .map(|u| (clave(u), format!("{} {}", MESES[u.month() as usize - 1], u.year())));

    let anterior = (inicio - Duration::days(1)).replace_day(1).unwrap();
    Ok(Pagina {
        titulo: format!("{} {}", MESES[inicio.month() as usize - 1], inicio.year()),
        mes: clave(inicio),
        anterior: clave(anterior),
        siguiente: clave(fin),
        ultimo_con_datos,
        semanas: semanas(inicio, &por_dia, &marcas, hoy, elegido),
        total: pesos(total),
        compras: por_dia.values().map(|v| v.1).sum(),
        dia_titulo: elegido.map(|d| format!("{} de {}", d.day(), MESES[d.month() as usize - 1].to_lowercase())).unwrap_or_default(),
        dia_compras,
        obligaciones: obligaciones(&regimenes, efirma_vence, hoy, 90),
    }
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // Vista previa con datos de ejemplo: AGENDA_HTML=/ruta.html cargo test vista_agenda -- --ignored
    #[test]
    #[ignore]
    fn vista_agenda() {
        let inicio = Date::from_calendar_date(2026, Month::October, 1).unwrap();
        let d = |n| inicio.replace_day(n).unwrap();
        let mut gasto = HashMap::new();
        for (n, m, c) in [(1, "5000", 2), (2, "350", 3), (5, "1200", 4), (14, "11000", 1), (20, "89", 1), (22, "640", 2)] {
            gasto.insert(d(n), (Decimal::from_str(m).unwrap(), c));
        }
        let p = Pagina {
            titulo: "Octubre 2026".into(),
            mes: "2026-10".into(),
            anterior: "2026-09".into(),
            siguiente: "2026-11".into(),
            ultimo_con_datos: None,
            semanas: semanas(inicio, &gasto, &[(d(17), "Pago mensual".into())], d(14), Some(d(5))),
            total: "$18,279".into(),
            compras: 13,
            dia_titulo: "5 de octubre".into(),
            dia_compras: vec![Compra { descripcion: "7 ELEVEN LOMAS DE SOTE".into(), categoria: "Tiendas de conveniencia".into(), cargo: "$45.00".into(), abono: String::new(), cuenta: true }],
            obligaciones: vec![],
        };
        std::fs::write(std::env::var("AGENDA_HTML").unwrap(), p.render().unwrap()).unwrap();
    }

    #[test]
    fn cuadricula_del_mes() {
        // Octubre 2026 empieza en jueves: 3 celdas vacías antes del 1.
        let inicio = Date::from_calendar_date(2026, Month::October, 1).unwrap();
        let d = |n| inicio.replace_day(n).unwrap();
        let mut gasto = HashMap::new();
        gasto.insert(d(5), (Decimal::from_str("1500").unwrap(), 3));
        gasto.insert(d(6), (Decimal::from_str("750").unwrap(), 1));
        let obligs = vec![(d(17), "Pago mensual".to_string())];
        let s = semanas(inicio, &gasto, &obligs, d(6), Some(d(5)));

        assert_eq!(s.len(), 5);
        assert!(s.iter().all(|w| w.len() == 7));
        assert!(s[0][..3].iter().all(Option::is_none));
        let dia = |n: u8| s.iter().flatten().flatten().find(|x| x.numero == n).unwrap();
        assert_eq!(dia(1).gasto, "");
        assert_eq!((dia(5).gasto.as_str(), dia(5).nivel, dia(5).elegido), ("$1.5k", 100, true));
        assert_eq!((compacto(Decimal::from(5000)), compacto(Decimal::from(11000)), compacto(Decimal::from(89))), ("$5k".into(), "$11k".into(), "$89".into()));
        assert_eq!((dia(6).gasto.as_str(), dia(6).nivel, dia(6).hoy), ("$750", 50, true));
        assert_eq!(dia(17).obligaciones, vec!["Pago mensual".to_string()]);
        assert_eq!(s.iter().flatten().flatten().count(), 31);
    }
}
