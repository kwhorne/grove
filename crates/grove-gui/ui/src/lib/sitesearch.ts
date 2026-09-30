// Finding a site among many: the Sites filter, the ⌘K palette's search, and
// "what was I working on" for the palette's opening list.

import type { ResolvedSite } from "./types";

/** The text a site is searched by: its hostname first, then where it lives. */
function haystack(s: ResolvedSite): string {
  return `${s.hostname} ${s.path} ${s.driver} ${s.php}`.toLowerCase();
}

/**
 * The Sites filter: every word typed must appear somewhere in the site's
 * hostname, path, driver or PHP version. Plain substrings, so what shows is
 * what you would expect from reading it.
 */
export function matchesFilter(s: ResolvedSite, filter: string): boolean {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const h = haystack(s);
  return words.every((w) => h.includes(w));
}

/**
 * How well `query` fuzzy-matches `text`: its letters in order, not
 * necessarily together. Higher is better; `null` is no match. Runs of
 * consecutive letters, a match at the start and matches at word boundaries
 * (after `-`, `_`, `.`, `/`, a space, or a lower→upper change) score more, so
 * "ask" prefers `askr.test` to `mask-rater.test`, and "abn" finds
 * `abonnementsoversikt.test`.
 */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const t = text.toLowerCase();
  let score = 0;
  let ti = 0;
  let run = 0;
  for (let qi = 0; qi < q.length; qi++) {
    const c = q[qi];
    const found = t.indexOf(c, ti);
    if (found < 0) return null;
    if (found === ti && qi > 0) {
      run += 1;
      score += 4 * run;
    } else {
      run = 0;
      score -= Math.min(found - ti, 8);
    }
    const prev = found === 0 ? "" : text[found - 1];
    const here = text[found];
    if (found === 0) score += 12;
    else if (/[-_./ ]/.test(prev)) score += 8;
    else if (prev === prev.toLowerCase() && here !== here.toLowerCase()) score += 6;
    ti = found + 1;
  }
  // Shorter names that match as well are the likelier ones.
  return score - text.length * 0.05;
}

/** A site's best fuzzy score: by hostname, or failing that by path. */
export function siteScore(s: ResolvedSite, query: string): number | null {
  const byHost = fuzzyScore(query, s.hostname);
  if (byHost !== null) return byHost + 10;
  return fuzzyScore(query, s.path);
}

/** Why a site counts as recent, for the palette to say. */
export type RecentReason = "git" | "request" | "picked";

export interface Recent {
  site: ResolvedSite;
  at: number;
  reason: RecentReason;
}

const PICKED_KEY = "grove.palette.picked";

/** Sites chosen in the palette, by name, as unix ms. */
export function loadPicked(): Record<string, number> {
  try {
    const raw = localStorage.getItem(PICKED_KEY);
    const v = raw ? JSON.parse(raw) : {};
    return v && typeof v === "object" ? v : {};
  } catch {
    return {};
  }
}

export function rememberPicked(name: string): void {
  try {
    const picked = loadPicked();
    picked[name] = Date.now();
    // Keep it small: the newest 50 are more than the palette ever shows.
    const kept = Object.entries(picked)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 50);
    localStorage.setItem(PICKED_KEY, JSON.stringify(Object.fromEntries(kept)));
  } catch {
    /* storage unavailable: the palette still works, it just forgets */
  }
}

/**
 * The sites most recently worked on, newest first. A site counts from the
 * newest of three signals: activity in its git repo, the last request Grove
 * served it, and the last time it was chosen here.
 */
export function recentSites(
  sites: ResolvedSite[],
  git: Record<string, number>,
  requests: Record<string, number>,
  picked: Record<string, number>,
  limit = 10,
): Recent[] {
  const out: Recent[] = [];
  for (const site of sites) {
    const candidates: [number, RecentReason][] = [
      [git[site.path] ?? 0, "git"],
      [requests[site.name] ?? 0, "request"],
      [picked[site.name] ?? 0, "picked"],
    ];
    const [at, reason] = candidates.reduce((a, b) => (b[0] > a[0] ? b : a));
    if (at > 0) out.push({ site, at, reason });
  }
  out.sort((a, b) => b.at - a.at);
  return out.slice(0, limit);
}

/** `3m`, `2h`, `4d` ago. */
export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
