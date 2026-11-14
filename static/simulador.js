// Simulador de crecimiento de una inversión: interés compuesto mensual, todo en el navegador.
// Barras apiladas por año: monto inicial, lo depositado y el rendimiento ganado.
const forma = document.getElementById("simulador");
const svg = document.getElementById("sim-grafica");
const tip = document.getElementById("sim-tooltip");
const NS = "http://www.w3.org/2000/svg";
const W = 640, H = 300, M = { izq: 60, der: 8, arr: 12, aba: 28 };
// Tres tonos de un mismo color (de claro a oscuro), en el orden en que se apilan.
const CAPAS = [
  ["inicial", "Monto inicial", 0.3],
  ["aportado", "Lo que depositas", 0.6],
  ["rendimiento", "Rendimiento ganado", 1],
];

const pesos = (n) => "$" + Math.round(n).toLocaleString("es-MX");
const corto = (n) => (n >= 1e6 ? `$${+(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `$${Math.round(n / 1e3)}k` : `$${Math.round(n)}`);

function calcular({ inicial, aportacion, frecuencia, tasa, anios }) {
  const mensual = Math.pow(1 + tasa / 100, 1 / 12) - 1;
  let saldo = inicial, aportado = 0;
  const filas = [];
  for (let m = 0; m < anios * 12; m++) {
    // Depósito al inicio del periodo: cada mes, o cada 12 meses si es anual.
    if (frecuencia === 12 || m % 12 === 0) { saldo += aportacion; aportado += aportacion; }
    saldo *= 1 + mensual;
    if ((m + 1) % 12 === 0) filas.push({ anio: (m + 1) / 12, inicial, aportado, rendimiento: saldo - inicial - aportado, total: saldo });
  }
  return filas;
}

/// Paso "redondo" del eje (1, 2, 2.5 o 5 por potencia de 10) para 4 marcas que cubran el máximo.
function paso(max) {
  if (max <= 0) return 1;
  const crudo = max / 4, p = Math.pow(10, Math.floor(Math.log10(crudo)));
  return [1, 2, 2.5, 5, 10].map((k) => k * p).find((v) => v >= crudo);
}

function el(tipo, attrs, padre) {
  const e = document.createElementNS(NS, tipo);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  padre.append(e);
  return e;
}

/// Rectángulo con las esquinas de arriba redondeadas (el extremo de la barra).
function barraArriba(x, y, w, h, r) {
  r = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

function dibujar(filas) {
  svg.replaceChildren();
  const p = paso(Math.max(...filas.map((f) => f.total))), max = p * 4;
  const alto = H - M.arr - M.aba, ancho = W - M.izq - M.der;
  const y = (v) => M.arr + alto - (v / max) * alto;

  for (let i = 0; i <= 4; i++) {
    const v = p * i;
    el("line", { x1: M.izq, x2: W - M.der, y1: y(v), y2: y(v), stroke: "var(--line)", "stroke-width": 1 }, svg);
    el("text", { x: M.izq - 8, y: y(v) + 4, "text-anchor": "end", "font-size": 13, fill: "var(--muted)" }, svg).textContent = corto(v);
  }

  const banda = ancho / filas.length;
  const bw = Math.min(28, banda * 0.65);
  const cada = filas.length <= 12 ? 1 : 5;
  filas.forEach((f, i) => {
    const x = M.izq + banda * i + (banda - bw) / 2;
    const g = el("g", { tabindex: 0, "aria-label": `Año ${f.anio}: ${pesos(f.total)}` }, svg);
    let base = 0;
    const visibles = CAPAS.filter(([k]) => f[k] > 0);
    visibles.forEach(([k, , op], j) => {
      const y0 = y(base), y1 = y(base + f[k]);
      base += f[k];
      const h = Math.max(0, y0 - y1);
      // Separación de 2px entre capas con un trazo del color de la tarjeta.
      const attrs = { fill: "var(--primary)", "fill-opacity": op, stroke: "var(--card)", "stroke-width": 1 };
      if (j === visibles.length - 1) el("path", { d: barraArriba(x, y1, bw, h, 4), ...attrs }, g);
      else el("rect", { x, y: y1, width: bw, height: h, ...attrs }, g);
    });
    // Zona de hover más grande que la barra.
    const zona = el("rect", { x: M.izq + banda * i, y: M.arr, width: banda, height: alto, fill: "transparent" }, g);
    const mostrar = () => {
      tip.innerHTML = `<strong>Año ${f.anio}: ${pesos(f.total)}</strong><br>` +
        CAPAS.map(([k, nombre]) => `${nombre}: ${pesos(f[k])}`).join("<br>");
      tip.style.display = "block";
      const caja = svg.getBoundingClientRect(), escala = caja.width / W;
      const izq = (x + bw / 2) * escala;
      tip.style.left = Math.min(Math.max(0, izq - tip.offsetWidth / 2), caja.width - tip.offsetWidth) + "px";
      tip.style.top = Math.max(0, y(f.total) * escala - tip.offsetHeight - 8) + "px";
      svg.querySelectorAll("g").forEach((o) => o.setAttribute("opacity", o === g ? 1 : 0.45));
    };
    const ocultar = () => {
      tip.style.display = "none";
      svg.querySelectorAll("g").forEach((o) => o.setAttribute("opacity", 1));
    };
    zona.addEventListener("mouseenter", mostrar);
    zona.addEventListener("mouseleave", ocultar);
    g.addEventListener("focus", mostrar);
    g.addEventListener("blur", ocultar);
    if (f.anio === 1 || f.anio % cada === 0 || i === filas.length - 1) {
      el("text", { x: x + bw / 2, y: H - 8, "text-anchor": "middle", "font-size": 13, fill: "var(--muted)" }, svg).textContent = f.anio;
    }
  });
}

function tabla(filas) {
  const t = document.getElementById("sim-tabla");
  t.innerHTML = "<thead class='text-muted'><tr><th class='text-left py-1'>Año</th>" +
    CAPAS.map(([, n]) => `<th class='text-right py-1'>${n}</th>`).join("") + "<th class='text-right py-1'>Total</th></tr></thead><tbody>" +
    filas.map((f) => `<tr class='border-t border-line'><td class='py-1'>${f.anio}</td>` +
      CAPAS.map(([k]) => `<td class='text-right py-1'>${pesos(f[k])}</td>`).join("") +
      `<td class='text-right py-1 font-semibold'>${pesos(f.total)}</td></tr>`).join("") + "</tbody>";
}

function actualizar() {
  const d = new FormData(forma);
  const num = (k) => Math.max(0, parseFloat(d.get(k)) || 0);
  const anios = parseInt(d.get("anios"), 10);
  const tasa = d.get("tasa") === "otra" ? Math.min(50, num("otra")) : parseFloat(d.get("tasa"));
  document.getElementById("sim-anios-texto").textContent = anios;
  const filas = calcular({ inicial: num("inicial"), aportacion: num("aportacion"), frecuencia: parseInt(d.get("frecuencia"), 10), tasa, anios });
  const u = filas[filas.length - 1];
  const resumen = `En ${anios} años tendrías ${pesos(u.total)}: ${pesos(u.inicial + u.aportado)} que pusiste y ${pesos(u.rendimiento)} de rendimiento.`;
  document.getElementById("sim-resumen").innerHTML = resumen.replace(pesos(u.total), `<strong>${pesos(u.total)}</strong>`);
  svg.setAttribute("aria-label", resumen);
  dibujar(filas);
  tabla(filas);
}

forma.addEventListener("input", actualizar);
actualizar();
