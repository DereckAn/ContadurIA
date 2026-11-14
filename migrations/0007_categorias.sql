-- Categoría de gasto por comercio (nombre normalizado de la descripción del banco).
-- Guarda lo que clasificó la IA y lo que el usuario corrigió a mano; las reglas por
-- palabra clave viven en el código (src/gastos.rs).
CREATE TABLE categorias_comercio (
    usuario_id  UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    comercio    TEXT NOT NULL,
    categoria   TEXT NOT NULL,
    PRIMARY KEY (usuario_id, comercio)
);
