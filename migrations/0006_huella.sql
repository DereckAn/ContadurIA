-- Huella (sha256) del PDF de origen: re-subir el mismo archivo reemplaza sus movimientos,
-- aunque dos estados distintos se llamen igual ("estado.pdf").
ALTER TABLE movimientos ADD COLUMN huella TEXT;
