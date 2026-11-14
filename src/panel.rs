//! Panel (/): todo de un vistazo. Salud fiscal (semáforo, KPIs, tope de deducciones), tu dinero
//! (gastos, inversiones, banco) y un simulador de cuánto crecería tu inversión.

use askama::Template;
use askama_web::WebTemplate;
use axum::{extract::State, response::{IntoResponse, Redirect, Response}};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use time::Date;

use crate::{AppError, AppState, anual, auth::Usuario, calendario, gastos, inversiones};

fn pesos(n: Decimal) -> String {
    format!("${:.2}", n.round_dp(2))
}

/// Un foco del semáforo. nivel: "ok" | "alerta" | "peligro" | "info".
pub struct Chip {
    pub nivel: &'static str,
    pub texto: String,
}

impl Chip {
    pub fn icono(&self) -> &'static str {
        match self.nivel {
            "ok" => "✅",
            "alerta" => "⚠️",
            "peligro" => "🔴",
            _ => "ℹ️",
        }
    }
}

#[derive(Template, WebTemplate)]
#[template(path = "panel.html")]
pub struct Pagina {
    nombre: String,
    anio: i32,
    chips: Vec<Chip>,
    // KPIs
    ingreso: String,
    resultado_label: String,
    resultado_monto: String,
    cfdi_count: i64,
    proxima: String,
    // Tope de deducciones
    tope: String,
    tope_usado: String,
    tope_pct: i64,
    // Devolución
    mostrar_devolucion: bool,
    devolucion: String,
    // Tu dinero (None si aún no sube estados de cuenta)
    gastos: Option<gastos::Tablero>,
    inversion: Option<Inversion>,
    movimientos: i64,
    con_factura: i64,
    // Simulador: valores iniciales con tus números reales.
    sim_inicial: i64,
    sim_mensual: i64,
}

pub struct Inversion {
    pub total: String,
    pub pesos: String,
    pub dolares: String,
}

pub async fn pagina(State(st): State<AppState>, Usuario(id): Usuario) -> Result<Response, AppError> {
    // Necesita perfil; si no hay, a crearlo.
    let perfil: Option<(String, Vec<String>, Option<Date>)> =
        sqlx::query_as("SELECT nombre, regimenes, efirma_vence FROM perfiles WHERE usuario_id = $1")
            .bind(id)
            .fetch_optional(&st.db)
            .await?;
    let Some((nombre, regimenes, efirma_vence)) = perfil else {
        return Ok(Redirect::to("/perfil").into_response());
    };

    let hoy = crate::ahora_mx().date();
    let anio = hoy.year();
    let (c, _) = anual::calcular(&st.db, id, anio).await?;

    let cfdi_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM cfdis WHERE usuario_id = $1 AND EXTRACT(YEAR FROM fecha) = $2")
            .bind(id)
            .bind(anio)
            .fetch_one(&st.db)
            .await?;

    let sin_respaldo: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(abono),0) FROM movimientos
         WHERE usuario_id = $1 AND abono IS NOT NULL AND cfdi_uuid IS NULL AND EXTRACT(YEAR FROM fecha) = $2",
    )
    .bind(id)
    .bind(anio)
    .fetch_one(&st.db)
    .await?;

    // --- Semáforo ---
    let mut chips = Vec::new();
    let obligaciones = calendario::obligaciones(&regimenes, efirma_vence, hoy, 30);
    let vencidas = obligaciones.iter().filter(|o| o.faltan < 0).count();
    let pronto = obligaciones.iter().filter(|o| (0..=7).contains(&o.faltan)).count();
    if vencidas > 0 {
        chips.push(Chip { nivel: "peligro", texto: format!("Tienes {vencidas} obligación(es) vencida(s).") });
    } else if pronto > 0 {
        chips.push(Chip { nivel: "alerta", texto: format!("{pronto} obligación(es) vencen en los próximos 7 días.") });
    } else {
        chips.push(Chip { nivel: "ok", texto: "Estás al corriente con tus próximas obligaciones.".into() });
    }

    if c.resultado < Decimal::ZERO {
        chips.push(Chip { nivel: "info", texto: format!("Tienes {} a favor; puedes solicitar devolución.", pesos(-c.resultado)) });
    } else if c.resultado > Decimal::ZERO {
        chips.push(Chip { nivel: "alerta", texto: format!("Llevas {} de ISR a cargo estimado.", pesos(c.resultado)) });
    }

    if sin_respaldo > Decimal::ZERO {
        chips.push(Chip { nivel: "alerta", texto: format!("{} en depósitos del banco sin respaldo.", pesos(sin_respaldo)) });
    }

    if let Some(v) = efirma_vence {
        let dias = (v - hoy).whole_days();
        if dias < 0 {
            chips.push(Chip { nivel: "peligro", texto: "Tu e.firma está vencida.".into() });
        } else if dias <= 60 {
            chips.push(Chip { nivel: "alerta", texto: format!("Tu e.firma vence en {dias} días.") });
        }
    }

    let passkeys: i64 = sqlx::query_scalar("SELECT count(*) FROM passkeys WHERE usuario_id = $1")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    if passkeys < 2 {
        chips.push(Chip { nivel: "alerta", texto: "Sólo tienes una passkey: agrega otra en Seguridad para no perder tu cuenta.".into() });
    }
    let sat_en_proceso: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sat_solicitudes WHERE usuario_id = $1 AND estado IN ('solicitada', 'lista')",
    )
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    if sat_en_proceso > 0 {
        chips.push(Chip { nivel: "info", texto: format!("El SAT tiene {sat_en_proceso} descarga(s) de CFDI pendientes: vuelve a sincronizar.") });
    }

    // --- Tu dinero ---
    let gastos = gastos::tablero(&st, id).await?;
    let resumen = inversiones::resumen(&st.db, id).await?;
    let (movimientos, con_factura): (i64, i64) =
        sqlx::query_as("SELECT count(*), count(cfdi_uuid) FROM movimientos WHERE usuario_id = $1")
            .bind(id)
            .fetch_one(&st.db)
            .await?;
    let inversion = resumen.as_ref().map(|r| Inversion {
        total: gastos::pesos(r.total_mxn()),
        pesos: gastos::pesos(r.valor_mxn),
        dolares: if r.valor_usd.is_zero() { String::new() } else { format!("US${:.2}", r.valor_usd.round_dp(2)) },
    });
    let sim_inicial = resumen.as_ref().map_or(10_000, |r| r.total_mxn().round().to_i64().unwrap_or(0));
    let sim_mensual = resumen.as_ref().map_or(1_000, |r| r.aportacion_mensual.to_i64().unwrap_or(0));

    // --- KPIs ---
    let (resultado_label, resultado_monto) = if c.resultado < Decimal::ZERO {
        ("Saldo a favor".to_string(), pesos(-c.resultado))
    } else {
        ("ISR a cargo".to_string(), pesos(c.resultado))
    };
    let proxima = obligaciones
        .first()
        .map(|o| format!("{} — {}", o.titulo, o.faltan_texto()))
        .unwrap_or_else(|| "Sin obligaciones próximas".into());

    // --- Tope de deducciones ---
    let tope_pct = if c.tope > Decimal::ZERO {
        ((c.deducciones * Decimal::from(100)) / c.tope).round().to_i64().unwrap_or(0).clamp(0, 100)
    } else {
        0
    };

    Ok(Pagina {
        nombre,
        anio,
        chips,
        ingreso: pesos(c.ingreso),
        resultado_label,
        resultado_monto,
        cfdi_count,
        proxima,
        tope: pesos(c.tope),
        tope_usado: pesos(c.deducciones),
        tope_pct,
        mostrar_devolucion: c.resultado < Decimal::ZERO,
        devolucion: pesos(-c.resultado),
        gastos,
        inversion,
        movimientos,
        con_factura,
        sim_inicial,
        sim_mensual,
    }
    .into_response())
}
