-- Cuentas en dólares (GBM+ USD / DriveWealth): sus montos van en USD, no se suman a los de pesos.
ALTER TABLE inversiones ADD COLUMN moneda TEXT NOT NULL DEFAULT 'MXN';
