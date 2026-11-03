// Crear cuenta y entrar con passkey (WebAuthn). Requiere un navegador de 2024 o más nuevo.
const formulario = document.getElementById("acceso");
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

async function registrar(correo) {
  const reto = await enviar("/auth/registro/inicio", { correo });
  const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(reto.publicKey);
  const cred = await navigator.credentials.create({ publicKey });
  await enviar("/auth/registro/fin", cred.toJSON());
}

async function entrar(correo) {
  const reto = await enviar("/auth/acceso/inicio", { correo });
  const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(reto.publicKey);
  const cred = await navigator.credentials.get({ publicKey });
  await enviar("/auth/acceso/fin", cred.toJSON());
}

formulario.addEventListener("submit", async (e) => {
  e.preventDefault();
  const accion = e.submitter.value === "registro" ? registrar : entrar;
  mensaje.textContent = "";
  try {
    await accion(formulario.correo.value);
    location.href = "/";
  } catch (err) {
    mensaje.textContent = err.name === "NotAllowedError" ? "Se canceló la passkey." : err.message;
  }
});
