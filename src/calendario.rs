//! Calendario de obligaciones fiscales de personas físicas.

use time::{Date, Duration, Month, Weekday};

/// Regímenes que maneja la app, con su clave del SAT.
pub const REGIMENES: &[(&str, &str)] = &[
    ("605", "Sueldos y salarios"),
    ("606", "Arrendamiento"),
    ("612", "Actividades empresariales y profesionales"),
    ("625", "Plataformas tecnológicas"),
    ("626", "RESICO"),
];

/// Regímenes con pagos mensuales el día 17 del mes siguiente (LISR, LIVA).
const MENSUALES: &[(&str, &str)] = &[
    ("606", "Arrendamiento: ISR (e IVA si rentas locales comerciales)"),
    ("612", "Actividad empresarial y profesional: ISR e IVA"),
    ("625", "Plataformas: ISR e IVA, si las retenciones no son pago definitivo"),
    ("626", "RESICO: ISR e IVA"),
];

const MESES: [&str; 12] = [
    "enero", "febrero", "marzo", "abril", "mayo", "junio",
    "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre",
];

pub struct Obligacion {
    pub fecha: Date,
    pub faltan: i64,
    pub titulo: String,
    pub detalle: String,
}

impl Obligacion {
    pub fn fecha_texto(&self) -> String {
        format!("{} de {} de {}", self.fecha.day(), MESES[self.fecha.month() as usize - 1], self.fecha.year())
    }

    pub fn faltan_texto(&self) -> String {
        match self.faltan {
            0 => "hoy".into(),
            1 => "mañana".into(),
            n if n < 0 => format!("vencida hace {} días", -n),
            n => format!("en {n} días"),
        }
    }
}

/// Si cae en fin de semana, el plazo se recorre al siguiente día hábil (art. 12 CFF).
// ponytail: no considera días feriados ni los días adicionales por sexto dígito del RFC; agregar con el motor fiscal.
fn habil(mut d: Date) -> Date {
    while matches!(d.weekday(), Weekday::Saturday | Weekday::Sunday) {
        d += Duration::days(1);
    }
    d
}

fn dia(año: i32, mes: Month, dia: u8) -> Date {
    Date::from_calendar_date(año, mes, dia).expect("fecha fija válida")
}

/// Obligaciones desde `hoy`: pagos mensuales de los próximos `dias`, la próxima anual y el vencimiento de la e.firma.
pub fn obligaciones(regimenes: &[String], efirma_vence: Option<Date>, hoy: Date, dias: i64) -> Vec<Obligacion> {
    let hasta = hoy + Duration::days(dias);
    let mut v = Vec::new();
    let mut nueva = |fecha: Date, titulo: String, detalle: String| {
        v.push(Obligacion { fecha, faltan: (fecha - hoy).whole_days(), titulo, detalle });
    };

    let mensuales: Vec<&str> = MENSUALES
        .iter()
        .filter(|(c, _)| regimenes.iter().any(|r| r == c))
        .map(|(_, d)| *d)
        .collect();
    if !mensuales.is_empty() {
        let (mut año, mut mes) = (hoy.year(), hoy.month());
        loop {
            let vence = habil(dia(año, mes, 17));
            if vence > hasta {
                break;
            }
            if vence >= hoy {
                let anterior = mes.previous();
                let año_ant = if anterior == Month::December { año - 1 } else { año };
                let titulo = format!("Pagos provisionales de {} {año_ant}", MESES[anterior as usize - 1]);
                nueva(vence, titulo, mensuales.join("; "));
            }
            mes = mes.next();
            if mes == Month::January {
                año += 1;
            }
        }
    }

    if !regimenes.is_empty() {
        let mut año = hoy.year();
        if habil(dia(año, Month::April, 30)) < hoy {
            año += 1;
        }
        let detalle = if regimenes.iter().all(|r| r == "605") {
            "Sólo si te aplica: más de $400,000 de ingresos, dos patrones, o quieres deducir gastos personales para saldo a favor."
        } else {
            "Obligatoria con tus regímenes. Incluye deducciones personales."
        };
        nueva(habil(dia(año, Month::April, 30)), format!("Declaración anual {}", año - 1), detalle.into());
    }

    if let Some(vence) = efirma_vence {
        nueva(vence, "Vence tu e.firma".into(), "Renuévala antes; sin ella no puedes descargar CFDI ni firmar ante el SAT.".into());
    }

    v.sort_by_key(|o| o.fecha);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn r(codigos: &[&str]) -> Vec<String> {
        codigos.iter().map(|c| c.to_string()).collect()
    }

    #[test]
    fn calendario() {
        // RESICO desde el 3 nov 2026, 90 días: 17 nov, 17 dic, 17 ene (domingo → lunes 18) y la anual.
        let o = obligaciones(&r(&["626"]), None, date!(2026 - 11 - 03), 90);
        let fechas: Vec<Date> = o.iter().map(|o| o.fecha).collect();
        assert_eq!(fechas, [date!(2026 - 11 - 17), date!(2026 - 12 - 17), date!(2027 - 01 - 18), date!(2027 - 04 - 30)]);
        assert_eq!(o[0].titulo, "Pagos provisionales de octubre 2026");
        assert_eq!(o[2].titulo, "Pagos provisionales de diciembre 2026");
        assert_eq!(o[3].titulo, "Declaración anual 2026");

        // Sólo sueldos: sin pagos mensuales, la anual es opcional.
        let o = obligaciones(&r(&["605"]), Some(date!(2027 - 02 - 01)), date!(2026 - 11 - 03), 90);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].titulo, "Vence tu e.firma");
        assert!(o[1].detalle.starts_with("Sólo si te aplica"));

        // Después del 30 de abril, la próxima anual es la del año en curso.
        let o = obligaciones(&r(&["605"]), None, date!(2027 - 05 - 02), 30);
        assert_eq!(o[0].titulo, "Declaración anual 2027");
    }
}
