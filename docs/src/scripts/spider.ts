// Header mark: a spider hanging from a silk thread. Hovering drops it lower and
// leaving climbs it back up. The body rides the thread like a damped spring,
// and each leg is a stick with a mass at its tip held by a damped rotational
// spring, so the legs swing only because the body accelerates:
//
//   body:  ÿ = KS (y* − y) − CS ẏ                (y runs 0 at rest → 1 dropped)
//   leg:   θ̈ = −K (θ − θ₀) − C θ̇ − a·REACH / l · cos θ
//
// where a is the body's acceleration, l the leg's length in the mark's 64-unit
// viewBox, and REACH scales the jolt to match the tuned prototype.

const KS = 110;
const CS = 9;
const K = 500;
const C = 10;
const REACH = 120;
const DROP_PX = 26;
const STEP = 0.002;

type Leg = { el: SVGGElement; th0: number; th: number; w: number; len: number };

export function mountSpider(brand: HTMLElement): void {
	const hang = brand.querySelector<HTMLElement>('.brand-hang');
	if (!hang) return;
	const legs: Leg[] = [...brand.querySelectorAll<SVGGElement>('.leg')].map((el) => {
		const foot = el.querySelector('circle')!;
		const dx = Number(foot.getAttribute('cx')) - 32;
		const dy = Number(foot.getAttribute('cy')) - 32;
		const th0 = Math.atan2(dy, dx);
		return { el, th0, th: th0, w: 0, len: Math.hypot(dx, dy) };
	});
	const still = matchMedia('(prefers-reduced-motion: reduce)');

	let target = 0;
	let y = 0;
	let v = 0;
	let running = false;
	let last = 0;

	const render = () => {
		hang.style.setProperty('--drop', `${(y * DROP_PX).toFixed(2)}px`);
		for (const leg of legs) {
			leg.el.style.transform = `rotate(${(((leg.th - leg.th0) * 180) / Math.PI).toFixed(2)}deg)`;
		}
	};

	const settled = () =>
		Math.abs(target - y) < 1e-3 && Math.abs(v) < 1e-3 && legs.every((l) => Math.abs(l.th - l.th0) < 1e-3 && Math.abs(l.w) < 1e-2);

	const frame = (now: number) => {
		const dt = Math.min(0.05, (now - last) / 1000);
		last = now;
		for (let n = Math.ceil(dt / STEP), i = 0; i < n; i++) {
			const h = dt / n;
			const a = KS * (target - y) - CS * v;
			v += a * h;
			y += v * h;
			for (const leg of legs) {
				const alpha = -K * (leg.th - leg.th0) - C * leg.w - ((a * REACH) / leg.len) * Math.cos(leg.th);
				leg.w += alpha * h;
				leg.th += leg.w * h;
			}
		}
		if (settled()) {
			y = target;
			v = 0;
			for (const leg of legs) {
				leg.th = leg.th0;
				leg.w = 0;
			}
			running = false;
		}
		render();
		if (running) requestAnimationFrame(frame);
	};

	const go = (to: number) => {
		target = to;
		if (still.matches) {
			y = to;
			render();
			return;
		}
		if (running) return;
		running = true;
		last = performance.now();
		requestAnimationFrame(frame);
	};

	brand.addEventListener('pointerenter', () => go(1));
	brand.addEventListener('pointerleave', () => go(0));
	brand.addEventListener('focus', () => go(1));
	brand.addEventListener('blur', () => go(0));
}
