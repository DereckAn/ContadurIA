/** Tailwind (binario standalone, sin Node). Regenera el CSS con ./build-css.sh */
module.exports = {
  darkMode: 'class',
  content: ['./templates/**/*.html'],
  theme: {
    extend: {
      colors: {
        bg: 'var(--bg)', surface: 'var(--surface)', card: 'var(--card)',
        ink: 'var(--ink)', muted: 'var(--muted)', line: 'var(--line)',
        primary: 'var(--primary)', 'on-primary': 'var(--on-primary)', 'primary-soft': 'var(--primary-soft)',
        sand: 'var(--sand)', 'sand-soft': 'var(--sand-soft)',
        ok: 'var(--ok)', 'ok-soft': 'var(--ok-soft)',
        warn: 'var(--warn)', 'warn-soft': 'var(--warn-soft)',
        danger: 'var(--danger)', 'danger-soft': 'var(--danger-soft)',
      },
      fontFamily: { sans: ['Inter', 'system-ui', 'sans-serif'], mono: ['JetBrains Mono', 'monospace'] },
      borderRadius: { DEFAULT: '0.25rem', lg: '0.5rem', xl: '0.75rem' },
      maxWidth: { content: '1280px' },
    },
  },
};
