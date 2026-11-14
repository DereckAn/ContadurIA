-- Resumen mensual de cada contrato de inversión (estados de cuenta de GBM).
-- Una fila por contrato y mes: re-subir el mismo estado la reemplaza.
CREATE TABLE inversiones (
    usuario_id    UUID NOT NULL REFERENCES usuarios(id) ON DELETE CASCADE,
    contrato      TEXT NOT NULL,
    periodo       DATE NOT NULL,     -- primer día del mes
    valor_inicial NUMERIC NOT NULL,
    valor_final   NUMERIC NOT NULL,
    entradas      NUMERIC NOT NULL,  -- depósitos de efectivo
    salidas       NUMERIC NOT NULL,  -- retiros y compras de dólares
    intereses     NUMERIC NOT NULL,  -- "derechos patrimoniales": intereses y dividendos
    isr_retenido  NUMERIC NOT NULL,
    dolares_usd   NUMERIC NOT NULL,  -- dólares comprados en el mes
    dolares_mxn   NUMERIC NOT NULL,  -- lo que costaron en pesos
    archivo       TEXT NOT NULL,
    PRIMARY KEY (usuario_id, contrato, periodo)
);
