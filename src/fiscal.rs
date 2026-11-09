//! Motor fiscal: tarifa anual de ISR y topes, con valores oficiales 2026.
//!
//! Tarifa anual art. 152 LISR, Anexo 8 RMF 2026 (DOF 28-dic-2025). UMA 2026 (INEGI 1/26).
// ponytail: valores fijos de 2026. Cuando llegue otro ejercicio hay que agregar su tabla y
// elegir la del año; por eso las funciones reciben el año y hoy fallan si no es 2026.

use std::str::FromStr;

use rust_decimal::Decimal;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// (límite inferior, cuota fija, tasa sobre excedente) — tarifa anual 2026.
fn tarifa_2026() -> [(Decimal, Decimal, Decimal); 11] {
    [
        (d("0.01"), d("0"), d("0.0192")),
        (d("10135.12"), d("194.59"), d("0.0640")),
        (d("86022.12"), d("5051.37"), d("0.1088")),
        (d("151176.20"), d("12140.13"), d("0.1600")),
        (d("175735.67"), d("16069.64"), d("0.1792")),
        (d("210403.70"), d("22282.14"), d("0.2136")),
        (d("424353.98"), d("67981.92"), d("0.2352")),
        (d("668840.15"), d("125485.07"), d("0.3000")),
        (d("1276925.99"), d("307910.81"), d("0.3200")),
        (d("1702567.98"), d("444116.23"), d("0.3400")),
        (d("5107703.93"), d("1601862.46"), d("0.3500")),
    ]
}

/// Tarifa mensual art. 96 LISR 2026 (para pagos provisionales acumulados).
fn tarifa_mensual_2026() -> [(Decimal, Decimal, Decimal); 11] {
    [
        (d("0.01"), d("0"), d("0.0192")),
        (d("844.60"), d("16.22"), d("0.0640")),
        (d("7168.52"), d("420.95"), d("0.1088")),
        (d("12598.03"), d("1011.68"), d("0.1600")),
        (d("14644.65"), d("1339.14"), d("0.1792")),
        (d("17533.65"), d("1856.84"), d("0.2136")),
        (d("35362.84"), d("5665.16"), d("0.2352")),
        (d("55736.69"), d("10457.09"), d("0.3000")),
        (d("106410.51"), d("25659.23"), d("0.3200")),
        (d("141880.67"), d("37009.69"), d("0.3400")),
        (d("425642.00"), d("133488.54"), d("0.3500")),
    ]
}

/// ISR del periodo (pagos provisionales 612/606/625): tarifa mensual con límites y cuota
/// multiplicados por el número de meses acumulados. Año sin tabla = None.
pub fn isr_acumulado(anio: i32, base: Decimal, meses: u32) -> Option<Decimal> {
    if base <= Decimal::ZERO {
        return Some(Decimal::ZERO);
    }
    if anio != 2026 {
        return None;
    }
    let m = Decimal::from(meses.max(1));
    let tarifa = tarifa_mensual_2026();
    let (li, cuota, tasa) = tarifa.iter().rev().find(|(li, _, _)| base >= *li * m)?;
    Some((*cuota * m + (base - *li * m) * *tasa).round_dp(2))
}

/// ISR de RESICO (art. 113-E LISR): tasa fija sobre ingresos cobrados del mes. Tabla de ley (fija).
pub fn isr_resico(ingreso: Decimal) -> Decimal {
    let tasa = if ingreso <= d("25000") {
        d("0.0100")
    } else if ingreso <= d("50000") {
        d("0.0110")
    } else if ingreso <= d("83333.33") {
        d("0.0150")
    } else if ingreso <= d("208333.33") {
        d("0.0200")
    } else {
        d("0.0250")
    };
    (ingreso * tasa).round_dp(2)
}

/// UMA anual 2026.
pub fn uma_anual() -> Decimal {
    d("42794.64")
}

/// ISR anual del ejercicio 2026 sobre una base gravable. Año distinto = sin tabla (None).
pub fn isr_anual(anio: i32, base: Decimal) -> Option<Decimal> {
    if anio != 2026 || base <= Decimal::ZERO {
        return if base <= Decimal::ZERO { Some(Decimal::ZERO) } else { None };
    }
    let tarifa = tarifa_2026();
    let (li, cuota, tasa) = tarifa.iter().rev().find(|(li, _, _)| base >= *li)?;
    Some((*cuota + (base - *li) * *tasa).round_dp(2))
}

/// Tope de deducciones personales (art. 151 LISR): lo menor entre 5 UMA anuales y 15% del ingreso.
pub fn tope_deducciones(ingreso: Decimal) -> Decimal {
    let cinco_uma = uma_anual() * Decimal::from(5);
    let quince = (ingreso * d("0.15")).round_dp(2);
    cinco_uma.min(quince)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isr_tramos() {
        // Base en el 3er tramo: 5051.37 + (100000-86022.12)*0.1088 = 6572.16
        assert_eq!(isr_anual(2026, d("100000")), Some(d("6572.16")));
        // Tramo 6 (ingreso tipo del usuario): 22282.14 + (266678.01-210403.70)*0.2136
        assert_eq!(isr_anual(2026, d("266678.01")), Some(d("34302.33")));
        assert_eq!(isr_anual(2026, Decimal::ZERO), Some(Decimal::ZERO));
        assert_eq!(isr_anual(2025, d("100000")), None); // sin tabla de otro año
    }

    #[test]
    fn acumulado_y_resico() {
        // Un mes, base 10000 → tramo 3 mensual: 420.95 + (10000-7168.52)*0.1088 = 729.01
        assert_eq!(isr_acumulado(2026, d("10000"), 1), Some(d("729.02")));
        // Dos meses, base 20000: límites×2 → tramo 3: 420.95*2 + (20000-7168.52*2)*0.1088
        let r3 = d("420.95") * Decimal::from(2) + (d("20000") - d("7168.52") * Decimal::from(2)) * d("0.1088");
        assert_eq!(isr_acumulado(2026, d("20000"), 2), Some(r3.round_dp(2)));
        assert_eq!(isr_acumulado(2026, Decimal::ZERO, 3), Some(Decimal::ZERO));
        // RESICO: 30000 → 1.10%
        assert_eq!(isr_resico(d("30000")), d("330.00"));
        assert_eq!(isr_resico(d("20000")), d("200.00")); // 1.00%
    }

    #[test]
    fn tope() {
        // 15% de 100000 = 15000 < 5 UMA (213973.20) → gana el 15%
        assert_eq!(tope_deducciones(d("100000")), d("15000.00"));
        // Ingreso alto: 5 UMA manda
        assert_eq!(tope_deducciones(d("5000000")), uma_anual() * Decimal::from(5));
    }
}
