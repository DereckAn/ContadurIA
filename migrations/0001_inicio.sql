CREATE TABLE usuarios (
    id      UUID PRIMARY KEY,
    correo  TEXT NOT NULL UNIQUE,
    creado  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Una persona puede tener varias passkeys (celular, laptop, llave física).
CREATE TABLE passkeys (
    id          BIGSERIAL PRIMARY KEY,
    usuario_id  UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    datos       JSONB NOT NULL
);
CREATE INDEX passkeys_usuario ON passkeys(usuario_id);

CREATE TABLE perfiles (
    usuario_id    UUID PRIMARY KEY REFERENCES usuarios(id) ON DELETE CASCADE,
    nombre        TEXT NOT NULL,
    rfc           TEXT NOT NULL,
    regimenes     TEXT[] NOT NULL,  -- claves del SAT: 605, 606, 612, 625, 626
    efirma_vence  DATE
);
