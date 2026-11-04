-- Solicitudes al web service de Descarga Masiva del SAT.
CREATE TABLE sat_solicitudes (
    id_solicitud   TEXT PRIMARY KEY,
    usuario_id     UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    tipo           TEXT NOT NULL CHECK (tipo IN ('emitidos', 'recibidos')),
    fecha_inicial  TIMESTAMP NOT NULL,  -- hora de México, como se pidió al SAT
    fecha_final    TIMESTAMP NOT NULL,
    estado         TEXT NOT NULL DEFAULT 'solicitada'
                   CHECK (estado IN ('solicitada', 'lista', 'descargada', 'error')),
    mensaje        TEXT NOT NULL DEFAULT '',
    paquetes       TEXT[] NOT NULL DEFAULT '{}',
    descargados    TEXT[] NOT NULL DEFAULT '{}',
    verificada     TIMESTAMPTZ,
    creada         TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sat_solicitudes_usuario ON sat_solicitudes(usuario_id);

CREATE TABLE cfdis (
    usuario_id       UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    uuid             TEXT NOT NULL,
    emitido          BOOLEAN NOT NULL,  -- true: lo emitió el usuario; false: lo recibió
    fecha            TIMESTAMP NOT NULL,
    tipo             TEXT NOT NULL,     -- I ingreso, E egreso, P pago, N nómina, T traslado
    emisor_rfc       TEXT NOT NULL,
    emisor_nombre    TEXT NOT NULL,
    receptor_rfc     TEXT NOT NULL,
    receptor_nombre  TEXT NOT NULL,
    uso_cfdi         TEXT NOT NULL,
    metodo_pago      TEXT NOT NULL,
    forma_pago       TEXT NOT NULL,
    moneda           TEXT NOT NULL,
    subtotal         NUMERIC NOT NULL,
    total            NUMERIC NOT NULL,
    xml              TEXT NOT NULL,
    PRIMARY KEY (usuario_id, uuid)
);
CREATE INDEX cfdis_usuario_fecha ON cfdis(usuario_id, fecha DESC);
