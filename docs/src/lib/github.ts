// The repository's star count, fetched once at build time. Pages read fine
// without it, so any failure leaves it null.
import { REPO } from '../site';

let stars: number | null = null;
try {
	const api = REPO.replace('https://github.com/', 'https://api.github.com/repos/');
	const res = await fetch(api, {
		headers: { Accept: 'application/vnd.github+json' },
		signal: AbortSignal.timeout(5000),
	});
	if (res.ok) stars = (await res.json()).stargazers_count ?? null;
} catch {
	stars = null;
}

export const STARS = stars;

export const formatCount = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k` : `${n}`);
