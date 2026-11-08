-- Chat de Telegram de cada usuario, para enviarle alertas.
ALTER TABLE perfiles ADD COLUMN telegram_chat_id TEXT;
