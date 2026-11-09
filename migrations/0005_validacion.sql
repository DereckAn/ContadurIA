-- Estado del CFDI en el SAT (Vigente / Cancelado) y cuándo se consultó.
ALTER TABLE cfdis ADD COLUMN estado_sat TEXT;
ALTER TABLE cfdis ADD COLUMN validado_en TIMESTAMPTZ;
