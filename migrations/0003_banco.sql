-- Movimientos bancarios leídos de estados de cuenta en PDF.
CREATE TABLE movimientos (
    id          BIGSERIAL PRIMARY KEY,
    usuario_id  UUID NOT NULL REFERENCES usuarios(id),
    fecha       DATE NOT NULL,
    descripcion TEXT NOT NULL,
    rfc         TEXT,
    cargo       NUMERIC,            -- dinero que salió
    abono       NUMERIC,            -- dinero que entró
    archivo     TEXT NOT NULL,      -- nombre del PDF de origen
    cfdi_uuid   TEXT,               -- CFDI con el que se concilió, si hay
    creado      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX movimientos_usuario_fecha ON movimientos (usuario_id, fecha DESC);
