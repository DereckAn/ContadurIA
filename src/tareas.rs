//! Tareas en segundo plano: recordatorios por Telegram y el informe mensual por correo.
//! Corre una vez al día (~7:00 México).

use time::{Date, PrimitiveDateTime, Time};
use uuid::Uuid;

use crate::{AppError, AppState, ahora_mx, calendario, informe, telegram};

pub async fn programador(st: AppState) {
    loop {
        dormir_hasta_las_7().await;
        if let Err(e) = correr(&st, ahora_mx().date()).await {
            eprintln!("tareas: {}", e.1);
        }
    }
}

async fn dormir_hasta_las_7() {
    let ahora = ahora_mx();
    let siete = Time::from_hms(7, 0, 0).unwrap();
    let objetivo = if ahora.time() < siete {
        PrimitiveDateTime::new(ahora.date(), siete)
    } else {
        PrimitiveDateTime::new(ahora.date().next_day().unwrap_or(ahora.date()), siete)
    };
    let secs = (objetivo - ahora).whole_seconds().max(1) as u64;
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
}

/// Revisa a todos los usuarios: avisa de obligaciones cercanas y, el día 2, manda el informe.
// ponytail: alertas en días fijos {7,3,1,0}, sin tabla de "ya enviado"; un reinicio en uno de
// esos días podría repetir el aviso. Agregar un registro de envíos si molesta.
/// Fila de perfil para las tareas: (usuario, regímenes, vencimiento e.firma, chat de Telegram).
type FilaPerfil = (Uuid, Vec<String>, Option<Date>, Option<String>);

async fn correr(st: &AppState, hoy: Date) -> Result<(), AppError> {
    let perfiles: Vec<FilaPerfil> =
        sqlx::query_as("SELECT usuario_id, regimenes, efirma_vence, telegram_chat_id FROM perfiles")
            .fetch_all(&st.db)
            .await?;

    for (id, regimenes, efirma, chat) in perfiles {
        if let Some(chat) = &chat {
            for o in calendario::obligaciones(&regimenes, efirma, hoy, 8) {
                if matches!(o.faltan, 0 | 1 | 3 | 7) {
                    let msg = format!(
                        "🔔 <b>{}</b>\n{}\n📅 {} ({})",
                        o.titulo, o.detalle, o.fecha_texto(), o.faltan_texto()
                    );
                    let _ = telegram::enviar(&st.http, chat, &msg).await;
                }
            }
        }
        if hoy.day() == 2 {
            let _ = informe::enviar(&st.db, id, &None, hoy).await; // informe del mes pasado
        }
    }
    Ok(())
}
