// Passkeys (WebAuthn): crear cuenta, entrar y agregar una passkey más. Requiere un navegador de 2024 o más nuevo.
const formulario = document.getElementById("acceso");
const boton = document.getElementById("agregar-passkey");
const mensaje = document.getElementById("mensaje");

async function enviar(url, cuerpo) {
  const r = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(cuerpo),
  });
  if (!r.ok) throw new Error((await r.text()) || r.statusText);
  return r.status === 204 ? null : r.json();
}

async function crear(inicio, fin, cuerpo) {
  const reto = await enviar(inicio, cuerpo);
  const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(reto.publicKey);
  const cred = await navigator.credentials.create({ publicKey });
  await enviar(fin, cred.toJSON());
}

async function entrar(correo) {
  const reto = await enviar("/auth/acceso/inicio", { correo });
  const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(reto.publicKey);
  const cred = await navigator.credentials.get({ publicKey });
  await enviar("/auth/acceso/fin", cred.toJSON());
}

function error(err) {
  if (err.name === "NotAllowedError") return "Se canceló la passkey.";
  if (err.name === "InvalidStateError") return "Este dispositivo ya tiene una passkey de tu cuenta.";
  return err.message;
}

formulario?.addEventListener("submit", async (e) => {
  e.preventDefault();
  mensaje.textContent = "";
  try {
    const correo = formulario.correo.value;
    if (e.submitter.value === "registro") await crear("/auth/registro/inicio", "/auth/registro/fin", { correo });
    else await entrar(correo);
    location.href = "/";
  } catch (err) {
    mensaje.textContent = error(err);
  }
});

// Botón "agregar passkey" (Seguridad y recuperación de cuenta).
boton?.addEventListener("click", async () => {
  mensaje.textContent = "";
  try {
    await crear("/auth/agregar/inicio", "/auth/agregar/fin", {});
    location.href = boton.dataset.destino || location.href;
  } catch (err) {
    mensaje.textContent = error(err);
  }
});
