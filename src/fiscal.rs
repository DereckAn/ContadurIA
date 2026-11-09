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
    fn tope() {
        // 15% de 100000 = 15000 < 5 UMA (213973.20) → gana el 15%
        assert_eq!(tope_deducciones(d("100000")), d("15000.00"));
        // Ingreso alto: 5 UMA manda
        assert_eq!(tope_deducciones(d("5000000")), uma_anual() * Decimal::from(5));
    }
}
