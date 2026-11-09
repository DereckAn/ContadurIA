# Matriz de pruebas — ContadurIA

Guía para verificar manualmente que todo funciona y que cualquier usuario nuevo podría usar la app.

**Cómo usarla:** sigue cada caso en orden, marca la casilla si el resultado es el esperado.
Corre primero las pruebas automáticas:

```bash
cargo test        # lógica (parser banco, motor fiscal, calendario, etc.)
cargo clippy      # sin advertencias
```

Entra siempre por `http://localhost:3000` (no `127.0.0.1`, o las passkeys fallan).

Leyenda de datos de prueba: RFC del usuario `AEMD980715TY3`, e.firma en `~/.efirma/`.

---

## 1. Cuentas y acceso (passkeys)

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 1.1 | Crear cuenta | En `/entrar`, escribe un correo nuevo → "Crear cuenta" → confirma la passkey | Entra y llega a `/perfil` | ☐ |
| 1.2 | Correo duplicado | Intenta crear cuenta con un correo ya registrado | Error claro, no crea otra cuenta | ☐ |
| 1.3 | Cerrar sesión | Botón "Salir" | Vuelve a `/entrar` | ☐ |
| 1.4 | Entrar | "Entrar" con el correo registrado + passkey | Entra a su panel | ☐ |
| 1.5 | Acceso sin sesión | Abre `/anual` sin haber entrado | Redirige a `/entrar` | ☐ |
| 1.6 | Passkey de otro equipo | Intenta entrar desde otro navegador/perfil sin la passkey | No deja entrar | ☐ |

## 2. Perfil fiscal

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 2.1 | Guardar perfil | Nombre, RFC válido, marca régimen(es), fecha de e.firma → Guardar | Se guarda y se ve al recargar | ☐ |
| 2.2 | RFC inválido | Guarda con un RFC mal formado | Error, no guarda | ☐ |
| 2.3 | Cambiar régimen | Cambia de 605 a 626 y guarda | `/pagos` ahora muestra cálculo RESICO | ☐ |

## 3. SAT y CFDI

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 3.1 | e.firma en el navegador | En `/sat`, carga `.cer` y `.key` + contraseña → "Abrir e.firma y sincronizar" | La bitácora avanza; la e.firma NO viaja al servidor | ☐ |
| 3.2 | RFC no coincide | Carga una e.firma de otro RFC | Error antes de sincronizar | ☐ |
| 3.3 | Descarga de CFDI | Deja sincronizar | Aparecen solicitudes y, al terminar, CFDI en `/cfdi` | ☐ |
| 3.4 | Listado de CFDI | Abre `/cfdi` | Tabla con fecha, tipo, contraparte, uso, total | ☐ |
| 3.5 | Validar con el SAT | Botón "Validar con el SAT" | Cada CFDI muestra estado (Vigente/Cancelado) | ☐ |
| 3.6 | Estado vacío | Usuario nuevo abre `/cfdi` | Mensaje "Aún no hay CFDI" con enlace a `/sat` | ☐ |

## 4. Banco y conciliación

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 4.1 | Subir estado BBVA | En `/banco`, sube el PDF | Movimientos con fecha, descripción, RFC, cargo/abono correctos | ☐ |
| 4.2 | Totales cuadran | Compara con los totales del propio estado de cuenta | Suma de cargos y abonos coincide con el banco | ☐ |
| 4.3 | Re-subir mismo archivo | Sube otra vez el mismo PDF | No duplica movimientos | ☐ |
| 4.4 | Conciliación | Botón "Conciliar con CFDI" | Marca ✓ los movimientos con factura (RFC+monto o monto+fecha) | ☐ |
| 4.5 | Sin factura | Revisa un gasto sin CFDI | Queda sin conciliar (correcto) | ☐ |
| 4.6 | Otro banco (respaldo IA) | Sube un PDF de banco distinto | Usa el modelo local; extrae movimientos (menor exactitud) | ☐ |

## 5. Cálculos fiscales

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 5.1 | Resumen anual | `/resumen` | Ingresos, gastos e IVA por mes | ☐ |
| 5.2 | Declaración anual | `/anual` | Desglose ISR y resultado a favor / a cargo | ☐ |
| 5.3 | Expediente ZIP | "Descargar expediente" | ZIP con resumen.txt, cfdis.csv y carpeta xml/ | ☐ |
| 5.4 | IVA | `/iva` | Trasladado, acreditable y diferencia por mes (≈0 en 605) | ☐ |
| 5.5 | Pagos provisionales 605 | `/pagos` con régimen 605 | Mensaje "no aplican pagos provisionales" | ☐ |
| 5.6 | Pagos RESICO | Cambia a 626 y abre `/pagos` | Tabla con tasa fija sobre lo cobrado | ☐ |
| 5.7 | Pagos 612/606/625 | Cambia régimen y abre `/pagos` | Tarifa acumulada 2026, fecha límite día 17 | ☐ |
| 5.8 | Año sin tabla | `/anual?anio=2025` | Avisa que solo hay tarifa 2026 | ☐ |
| 5.9 | Simulador de deducciones | En `/anual`, escribe un monto (ej. 20000) → "Simular" | Muestra el nuevo resultado y el ISR que ahorras; respeta el tope | ☐ |
| 5.10 | Simular arriba del tope | Simula un monto mayor al tope restante | El ahorro se limita al tope, no crece de más | ☐ |
| 5.11 | Proyección de cierre | `/anual` del año en curso con meses parciales | Muestra cómo cerrará el año y sugiere actuar antes del 31 dic | ☐ |

## 6. Discrepancias

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 6.1 | Depósitos sin respaldo | `/discrepancias` | Lista los abonos sin CFDI que los justifique | ☐ |
| 6.2 | CFDI cancelados | Tras "Validar con el SAT" | Muestra los que ya no están vigentes | ☐ |
| 6.3 | Todo respaldado | Usuario con todo conciliado | Mensaje "Todos tus depósitos están respaldados" | ☐ |

## 7. Informe mensual y correo

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 7.1 | Ver informe | `/informe` | Resumen del mes pasado (ingresos, gastos, deducibles, IVA, obligaciones) | ☐ |
| 7.2 | Elegir mes | `/informe?mes=2026-10` | Muestra ese mes | ☐ |
| 7.3 | Enviar por correo | Botón "Enviarme este informe" | Llega el correo; la página confirma el envío | ☐ |
| 7.4 | SMTP mal configurado | Borra `SMTP_PASS` del `.env` y reinicia | Error claro, no truena | ☐ |

## 8. Telegram y tareas automáticas

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 8.1 | Vincular | `/start` al bot → `/telegram` → "Vincular" | Guarda el chat y manda saludo de confirmación | ☐ |
| 8.2 | Mensaje de prueba | Botón "Enviar mensaje de prueba" | Llega el mensaje | ☐ |
| 8.3 | Alertas de obligaciones | (programador diario ~7:00) | Avisa cuando faltan 7/3/1/0 días | ☐ |
| 8.4 | Informe automático | Día 2 de cada mes | Llega el informe por correo sin intervención | ☐ |

## 9. Multiusuario y privacidad (clave para abrir a otros)

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 9.1 | Aislamiento de datos | Crea 2 cuentas (A y B), sube datos en cada una | A nunca ve CFDI, banco ni perfil de B | ☐ |
| 9.2 | e.firma privada | Revisa en la pestaña de red del navegador al sincronizar | El `.key` nunca sale del navegador | ☐ |
| 9.3 | Sesión expira | Deja 7 días sin actividad (o borra la cookie) | Pide entrar de nuevo | ☐ |
| 9.4 | Onboarding nuevo | Un usuario desde cero: crear cuenta → perfil → SAT → banco | Puede llegar a ver su declaración sin ayuda | ☐ |
| 9.5 | RFC ajeno | Intenta sincronizar CFDI con un RFC distinto al del perfil | El servidor lo rechaza | ☐ |

## 10. Robustez

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 10.1 | PDF que no es estado de cuenta | Súbelo en `/banco` | Error claro, no truena | ☐ |
| 10.2 | Ollama apagado | Detén Ollama y sube un banco no-BBVA | Error claro pedible reintentar | ☐ |
| 10.3 | SAT caído | Intenta sincronizar/validar sin red | Error claro, la app sigue viva | ☐ |
| 10.4 | Montos grandes | CFDI/movimientos de millones | Se muestran y suman sin perder centavos | ☐ |

## 11. Panel y salud fiscal

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 11.1 | Semáforo | Abre `/panel` | Focos ✅/⚠️/🔴 según obligaciones, saldo, discrepancias y e.firma | ☐ |
| 11.2 | KPIs | Revisa las tarjetas | Ingresos, saldo a favor/cargo, CFDI del año y próxima obligación correctos | ☐ |
| 11.3 | Barra del tope | Revisa la barra de deducciones | Muestra usado vs tope con barra de progreso | ☐ |
| 11.4 | Recordatorio de devolución | Usuario con saldo a favor | Aparece el aviso verde con enlace a la declaración | ☐ |
| 11.5 | Obligación vencida | Usuario con una obligación pasada | Foco rojo "obligación vencida" | ☐ |
| 11.6 | Sin perfil | Usuario nuevo sin perfil abre `/panel` | Redirige a `/perfil` | ☐ |

## 12. Explicaciones en lenguaje claro

| # | Caso | Pasos | Resultado esperado | OK |
|---|------|-------|--------------------|----|
| 12.1 | Anual | Abre "¿Qué significa esto?" en `/anual` | Explica ISR causado, retenido, saldo a favor y deducciones | ☐ |
| 12.2 | IVA | Abre el detalle en `/iva` | Explica trasladado vs acreditable | ☐ |
| 12.3 | Pagos | Abre el detalle en `/pagos` | Explica qué son los pagos provisionales | ☐ |
| 12.4 | Discrepancias | Abre el detalle en `/discrepancias` | Explica la discrepancia fiscal (art. 91) | ☐ |

---

## Pendientes conocidos (no son fallas)

- Tickets con foto (lectura por visión): aún no, falta tener un ticket de ejemplo.
- Tarifas fiscales: solo ejercicio 2026 por ahora.
- Arrendamiento (606): aún sin la opción de deducción ciega del 35%.
- Pagos provisionales: no consideran retenciones ni PTU todavía.
```
