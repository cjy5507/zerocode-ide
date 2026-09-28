/* 시안 v4와 같은 모양의 합성 볼트 (t-12443).
 *
 * 우주 보기의 사진과 수는 시안(docs/design/knowledge-graph-3d-v4/)과 **같은 모양의 자료**에서 잰다:
 * 쪽 850(×배수) · 선 약 4.94배 · 큰 주제 18(96~12쪽) · 작은 묶음 12 · 홀로 선 쪽 21 · 연결 수 가운데값 7,
 * 활동 중인 주제 넷에 최근 고친 쪽이 몰린 나이. 시안의 `kg-data.js`(`makeGraph`와 `universe`의 나이)를
 * 옮긴 것이다 — 제목·본문은 지어낸 말이고, 닮은 것은 수뿐이다. 창의 합성 볼트 빌더
 * (`window.__buildVaultGraph__`, knowledge-fixture.mjs)가 받는 모양(`customEdges`·`titles`·`modifiedMs`)으로
 * 낸다: 선언 관계는 이름 있는 관계(`related`·`implements`…)이고 추론은 본문 링크(`mentions`)다. */

export const seedUniverseVault = (target) => target.addInitScript(() => {
  const TOPICS = ["렌더링", "검색", "레이아웃", "기억", "빌드", "권한", "알림", "동기화", "편집기",
    "캐시", "시험", "배포", "기록", "설정", "접근성", "색 체계", "단축키", "문서"];
  const SIZES = [96, 84, 71, 66, 58, 52, 47, 46, 41, 38, 33, 29, 25, 22, 20, 18, 14, 12];
  const GROUPS = [8, 7, 6, 6, 5, 5, 4, 4, 3, 3, 3, 3];
  const ORPHANS = 21;
  const HEAD = ["프레임", "버퍼", "문맥", "입력", "창", "탭", "선", "점", "카드", "기록", "대기열", "잠금",
    "경로", "타이머", "색표", "글꼴", "그림자", "격자", "주소", "목록", "표", "단추", "창틀", "커서", "신호",
    "토큰", "묶음", "지도", "이름표", "사진"];
  const TAIL = ["의 수명", "{이} 늦는 까닭", "{을} 나누는 법", "{과} 그림자", "의 경계", "{을} 다시 쓸 때",
    "{이} 겹칠 때", "의 상한", "{을} 재는 법", "{이} 사라질 때", "의 첫 그림", "{을} 비우는 순서",
    "{과} 문맥 잃음", "의 두 가지 길", "{이} 멈추는 자리", "{을} 고르는 기준", "의 되돌리기", "{과} 다음 할 일",
    "{이} 흔들릴 때", "의 이름 짓기", "{을} 옮기는 차례", "의 작은 규칙", "{이} 쌓일 때", "{을} 묶는 방법",
    "의 빈자리", "{과} 성능", "{이} 틀린 날", "의 기본값", "{을} 가리는 법", "의 마지막 쪽"];
  /* 시안의 관계 여섯(자리가 곧 코드) — 0 선언된 `related`, 1 추론(본문 링크). */
  const KINDS = ["related", "mentions", "implements", "depends_on", "contradicts", "supersedes"];
  const prng = (seed) => {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  };
  const josa = (word, pair) => {
    const code = word.charCodeAt(word.length - 1);
    return code >= 0xac00 && code <= 0xd7a3 && (code - 0xac00) % 28 !== 0 ? pair[0] : pair[1];
  };
  const titleOf = (head, tail) => head + tail.replace("{이}", josa(head, "이가")).replace("{을}", josa(head, "을를"))
    .replace("{과}", josa(head, "과와"));
  const pick = (cum, u) => {
    let lo = 0;
    let hi = cum.length - 1;
    const x = u * cum[hi];
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (cum[mid] < x) lo = mid + 1; else hi = mid;
    }
    return lo;
  };
  window.__universeVaultSpec__ = (pages = 850, nowMs = Date.now()) => {
    const R = prng(20260928);
    const f = pages / 850;
    const topicsPlan = SIZES.map((size) => Math.max(4, Math.round(size * f)));
    const groupsPlan = GROUPS.map((size) => Math.max(2, Math.round(size * f)));
    const orphans = Math.max(1, Math.round(ORPHANS * f));
    topicsPlan[0] += pages - (topicsPlan.reduce((a, b) => a + b, 0) + groupsPlan.reduce((a, b) => a + b, 0) + orphans);
    const clusters = [];
    let at = 0;
    topicsPlan.forEach((size) => { clusters.push({ kind: "topic", size, start: at }); at += size; });
    groupsPlan.forEach((size) => { clusters.push({ kind: "group", size, start: at }); at += size; });
    clusters.push({ kind: "orphan", size: orphans, start: at });
    const n = at + orphans;
    const T = topicsPlan.length;
    const cluster = new Int32Array(n);
    for (let c = 0; c < clusters.length; c += 1) {
      for (let i = 0; i < clusters[c].size; i += 1) cluster[clusters[c].start + i] = c;
    }
    const combos = new Int32Array(HEAD.length * TAIL.length);
    for (let i = 0; i < combos.length; i += 1) combos[i] = i;
    for (let i = combos.length - 1; i > 0; i -= 1) {
      const j = Math.floor(R() * (i + 1));
      [combos[i], combos[j]] = [combos[j], combos[i]];
    }
    const titles = [];
    for (let i = 0; i < n; i += 1) {
      const c = combos[i % combos.length];
      const lap = Math.floor(i / combos.length);
      titles.push(titleOf(HEAD[c % HEAD.length], TAIL[Math.floor(c / HEAD.length)]) + (lap ? ` ${lap + 1}` : ""));
    }
    const aff = new Float32Array(T * T);
    const lead = new Float32Array(T * T);
    for (let a = 0; a < T; a += 1) for (let b = 0; b < T; b += 1) if (a !== b) aff[a * T + b] = 0.05 + R() * 0.08;
    for (let a = 0; a < T; a += 1) {
      const partners = 2 + (R() < 0.5 ? 1 : 0);
      for (let p = 0; p < partners; p += 1) {
        let b = Math.floor(R() * T);
        if (b === a) b = (b + 1) % T;
        const w = 0.9 + R() * 1.6;
        aff[a * T + b] += w;
        aff[b * T + a] += w;
      }
    }
    for (let a = 0; a < T; a += 1) {
      for (let b = a + 1; b < T; b += 1) {
        const fwd = R() < 0.5 ? 0.8 : 0.2;
        lead[a * T + b] = fwd;
        lead[b * T + a] = 1 - fwd;
      }
    }
    const affCum = [];
    for (let a = 0; a < T; a += 1) {
      const row = new Float64Array(T);
      let sum = 0;
      for (let b = 0; b < T; b += 1) { sum += aff[a * T + b]; row[b] = sum; }
      affCum.push(row);
    }
    const zipf = clusters.map((k) => {
      const cum = new Float64Array(k.size);
      let sum = 0;
      for (let i = 0; i < k.size; i += 1) { sum += 1 / (i + 1) ** 1.08; cum[i] = sum; }
      return cum;
    });
    const target = Math.round(pages * 4.94);
    const edges = [];
    const seen = new Set();
    const add = (a, b) => {
      if (a === b || edges.length >= target + 64) return;
      const key = a < b ? a * n + b : b * n + a;
      if (seen.has(key)) return;
      seen.add(key);
      let kind = 1;
      if (R() < 0.58) {
        const u = R();
        kind = u < 0.014 ? 2 : u < 0.024 ? 3 : u < 0.028 ? 4 : u < 0.0305 ? 5 : 0;
      }
      edges.push({ from: a, to: b, kind: KINDS[kind] });
    };
    const topicPages = topicsPlan.reduce((a, b) => a + b, 0);
    for (let c = 0; c < T; c += 1) {
      const k = clusters[c];
      for (let i = 1; i < k.size; i += 1) {
        const j = pick(zipf[c], R() * (zipf[c][i - 1] / zipf[c][k.size - 1]));
        add(k.start + i, k.start + Math.min(j, i - 1));
      }
    }
    for (let c = T; c < clusters.length - 1; c += 1) {
      const k = clusters[c];
      for (let i = 1; i < k.size; i += 1) add(k.start + i, k.start + Math.floor(R() * i));
      if (k.size > 3) add(k.start + k.size - 1, k.start);
    }
    const sizeCum = new Float64Array(T);
    for (let c = 0, acc = 0; c < T; c += 1) { acc += clusters[c].size; sizeCum[c] = acc; }
    let guard = 0;
    while (edges.length < target && guard < target * 20) {
      guard += 1;
      let s = Math.floor(R() * topicPages);
      if (R() < 0.4) { const c0 = pick(sizeCum, R()); s = clusters[c0].start + pick(zipf[c0], R()); }
      const cs = cluster[s];
      let ct = cs;
      if (R() > 0.7) ct = pick(affCum[cs], R());
      const t = clusters[ct].start + pick(zipf[ct], R());
      if (ct !== cs && R() > lead[cs * T + ct]) add(t, s); else add(s, t);
    }
    /* 나이(시안 `universe` 1): 활동 중인 주제 넷에 최근 고친 쪽이 몰려 있다. */
    const A = prng(7070707);
    const active = new Set([1, 3, 6, 10]);
    const modifiedMs = new Array(n);
    for (let v = 0; v < n; v += 1) {
      const k = clusters[cluster[v]];
      const hot = k.kind === "topic" ? (active.has(cluster[v]) ? 0.36 : 0.05) : k.kind === "group" ? 0.14 : 0;
      let age = A() < hot ? A() * 9 : 9 + -Math.log(1 - A() * 0.999) * 170;
      if (k.kind === "orphan") age = 90 + A() * 900;
      modifiedMs[v] = Math.round(nowMs - age * 86_400_000);
    }
    return { pages: n, ghosts: 0, tags: [], customEdges: edges, titles, modifiedMs };
  };
});
