// Tiny, dependency-free SQL syntax highlighter. Returns HTML (already escaped)
// with <span class="tok-*"> wrappers, used behind a transparent textarea so the
// query editor stays a plain, accessible textarea while showing colors.

const KEYWORDS = new Set(
  (
    "select insert update delete from where and or not null is in like between " +
    "join inner left right outer full cross on group by having order asc desc " +
    "limit offset union all distinct as into values set create table alter drop " +
    "truncate index view primary key foreign references default unique constraint " +
    "add column auto_increment engine begin commit rollback transaction case when " +
    "then else end exists count sum avg min max coalesce cast convert with " +
    "returning using natural pragma explain describe desc show databases tables " +
    "int integer bigint varchar text char boolean bool date datetime timestamp " +
    "float double decimal serial uuid json jsonb blob"
  ).split(/\s+/),
);

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// Order matters: comments and strings first so keywords inside them aren't colored.
const TOKEN = new RegExp(
  [
    "(--[^\\n]*|/\\*[\\s\\S]*?\\*/)", // 1: comment
    "('(?:[^']|'')*'|\"(?:[^\"]|\"\")*\"|`[^`]*`)", // 2: string / quoted ident
    "(\\b\\d+(?:\\.\\d+)?\\b)", // 3: number
    "([A-Za-z_][A-Za-z0-9_]*)", // 4: word
    "([(),.;*=<>!+\\-/%|]+)", // 5: punctuation / operators
  ].join("|"),
  "g",
);

export function highlightSql(sql: string): string {
  let out = "";
  let last = 0;
  for (const m of sql.matchAll(TOKEN)) {
    const i = m.index ?? 0;
    if (i > last) out += escapeHtml(sql.slice(last, i));
    const raw = m[0];
    const esc = escapeHtml(raw);
    if (m[1]) out += `<span class="tok-comment">${esc}</span>`;
    else if (m[2]) out += `<span class="tok-string">${esc}</span>`;
    else if (m[3]) out += `<span class="tok-number">${esc}</span>`;
    else if (m[4]) {
      out += KEYWORDS.has(raw.toLowerCase())
        ? `<span class="tok-keyword">${esc}</span>`
        : esc;
    } else if (m[5]) out += `<span class="tok-punct">${esc}</span>`;
    else out += esc;
    last = i + raw.length;
  }
  if (last < sql.length) out += escapeHtml(sql.slice(last));
  // Trailing newline needs a placeholder so the <pre> height matches the textarea.
  return out + "\n";
}
