// Sincronización con el SAT: la e.firma se abre y firma aquí (WebAssembly);
// el servidor sólo reenvía al SAT los mensajes ya firmados.
// Import dinámico: si falta el WebAssembly, el submit igual queda interceptado y se muestra
// el error. Con un import estático todo el módulo fallaba y el navegador mandaba el formulario
// por GET, con la contraseña de la e.firma en la URL.
let Efirma = null;
async function cargarEfirma() {
  if (Efirma) return;
  try {
    const m = await import("/static/efirma/efirma.js");
    await m.default();
    Efirma = m.Efirma;
  } catch {
    throw new Error("No se encontró el módulo de e.firma (static/efirma). Corre ./build-wasm.sh y recarga.");
  }
}

const formulario = document.getElementById("efirma");
const bitacora = document.getElementById("bitacora");
let efirma = null;
let ocupado = false;

const anotar = (texto) => {
  const li = document.createElement("li");
  li.textContent = texto;
  bitacora.append(li);
};

class SatVencido extends Error {}

async function enviar(xml) {
  const r = await fetch("/sat/enviar", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ xml }),
  });
  const texto = await r.text();
  if (r.status === 401) throw new SatVencido(texto);
  if (!r.ok) throw new Error(texto);
  return JSON.parse(texto).mensaje;
}

async function autenticar() {
  const iso = (d) => d.toISOString().slice(0, 19) + ".000Z";
  const ahora = new Date();
  const expira = new Date(ahora.getTime() + 5 * 60 * 1000);
  return enviar(efirma.autenticacion(iso(ahora), iso(expira), `uuid-${crypto.randomUUID()}-1`));
}

function firmar(t) {
  if (t.accion === "solicitar") return efirma.solicitud(t.tipo, t.inicio, t.fin);
  if (t.accion === "verificar") return efirma.verificacion(t.id);
  return efirma.descarga(t.id);
}

async function sincronizar() {
  if (ocupado) return;
  ocupado = true;
  try {
    anotar(await autenticar());
    // Hasta 3 rondas: solicitar, verificar y descargar lo que ya esté listo.
    // Un error en una tarea no detiene las demás; sin avances en una ronda, termina.
    for (let ronda = 0; ronda < 3; ronda++) {
      const tareas = await (await fetch("/sat/pendientes")).json();
      let avances = 0;
      for (const t of tareas) {
        try {
          try {
            anotar(await enviar(firmar(t)));
          } catch (e) {
            if (!(e instanceof SatVencido)) throw e;
            await autenticar();
            anotar(await enviar(firmar(t)));
          }
          avances++;
        } catch (e) {
          anotar("Error: " + (e.message ?? e));
        }
      }
      if (!avances) break;
    }
    anotar("Sincronización terminada. Recarga la página para ver el estado.");
  } catch (e) {
    anotar("Error: " + (e.message ?? e));
  } finally {
    ocupado = false;
  }
}

formulario.addEventListener("submit", async (e) => {
  e.preventDefault();
  try {
    await cargarEfirma();
    const [cer, key] = await Promise.all([formulario.cer.files[0].arrayBuffer(), formulario.key.files[0].arrayBuffer()]);
    efirma = new Efirma(new Uint8Array(cer), new Uint8Array(key), formulario.contrasena.value);
    formulario.contrasena.value = "";
    if (efirma.rfc !== formulario.dataset.rfc) {
      throw new Error(`La e.firma es de ${efirma.rfc} y tu perfil dice ${formulario.dataset.rfc}.`);
    }
    anotar(`e.firma de ${efirma.rfc}, vigente hasta el ${new Date(efirma.vence).toLocaleDateString("es-MX")}.`);
    await sincronizar();
  } catch (err) {
    efirma = null;
    anotar("Error: " + (err.message ?? err));
  }
});

// Mientras la página siga abierta con la e.firma cargada, revisa lo pendiente.
setInterval(() => efirma && sincronizar(), 2 * 60 * 1000);
