// Проверка переводов: у всех языков те же ключи и параметры, что у en, и есть все ключи,
// которые используют окно (src/ui/index.html) и Rust (src/*.rs). Запуск: node scripts/check-i18n.mjs
import { readFileSync, readdirSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const { I18N, LANG_NAMES } = require('../src/ui/i18n.js');
const errors = [];
const en = I18N.en;
const params = s => [...s.matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort().join(',');

for (const [lang, dict] of Object.entries(I18N)) {
  if (!LANG_NAMES[lang]) errors.push(`${lang}: no name in LANG_NAMES`);
  for (const k of Object.keys(en)) {
    if (!(k in dict)) errors.push(`${lang}: missing ${k}`);
    else if (params(dict[k]) !== params(en[k])) errors.push(`${lang}.${k}: params {${params(dict[k])}} ≠ en {${params(en[k])}}`);
  }
  for (const k of Object.keys(dict)) if (!(k in en)) errors.push(`${lang}: extra ${k}`);
}

const rust = readFileSync('src/i18n.rs', 'utf8').match(/LANGUAGES: \[&str; \d+\] = \[([^\]]+)\]/)[1];
const rustLangs = [...rust.matchAll(/"(\w+)"/g)].map(m => m[1]);
if (rustLangs.join() !== Object.keys(I18N).join()) errors.push(`i18n.rs LANGUAGES ${rustLangs} ≠ I18N ${Object.keys(I18N)}`);

const html = readFileSync('src/ui/index.html', 'utf8');
const used = new Set();
for (const re of [/data-i18n(?:-title)?="(\w+)"/g, /data-label="(\w+)"/g, /\bt\('(\w+)'/g, /t\(swap \? '(\w+)' : '(\w+)'\)/g, /t\(is360Mode \? '(\w+)' : '(\w+)'\)/g]) {
  for (const m of html.matchAll(re)) m.slice(1).forEach(k => k && used.add(k));
}
for (const f of readdirSync('src').filter(f => f.endsWith('.rs'))) {
  const src = readFileSync('src/' + f, 'utf8');
  for (const m of src.matchAll(/"((?:st|err|warn|proc|upd)[A-Z]\w*)"/g)) used.add(m[1]);
}
for (const k of used) if (!(k in en)) errors.push(`key used but not translated: ${k}`);

if (errors.length) { console.error(errors.join('\n')); process.exit(1); }
console.log(`i18n ok: ${Object.keys(I18N).length} languages × ${Object.keys(en).length} keys, ${used.size} keys used`);
