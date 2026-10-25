/* A signed-in synthetic SPA. All state stays in this tab; no external service. */
(() => {
  const view = document.querySelector('#view');
  const uploader = document.querySelector('#uploader');
  const today = new Date();
  const iso = (date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
  const previous = iso(new Date(today.getFullYear(), today.getMonth() - 1, 15));
  const state = { date: previous, amount: '12000', title: '주차비 환급', template: 'refund', files: [], approver: '', submitted: false };
  const escape = (value) => String(value).replace(/[&<>"']/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char]);
  const filesText = () => state.files.map((file) => `${file.name} (${file.size} B)`).join(', ') || '첨부 없음';
  const navigate = (route) => { location.hash = route; };
  document.querySelector('#approval-menu').onclick = (event) => {
    const menu = document.querySelector('#menu-items');
    menu.hidden = !menu.hidden;
    event.target.setAttribute('aria-expanded', String(!menu.hidden));
  };
  // Direct links arrive before the SPA's menu context. Navigate by the menu.
  history.replaceState(null, '', `${location.pathname}${location.search}#home`);
  const render = () => {
    const route = location.hash.slice(1);
    if (route === 'documents') {
      view.innerHTML = `<h1>내 문서함</h1><label>기간<select id="period"><option value="current">이번 달</option><option value="previous">지난달</option></select></label><section id="documents" aria-live="polite">이번 달 문서 없음</section>`;
      document.querySelector('#period').onchange = (event) => {
        document.querySelector('#documents').innerHTML = event.target.value === 'previous'
          ? `<h2>지난달 완료 문서</h2><p>${previous} · 주차비 환급 · 12,000원 · 결재 완료</p><button id="open-document">문서 열기</button>` : '이번 달 문서 없음';
        const open = document.querySelector('#open-document');
        if (open) open.onclick = () => navigate('document');
      };
    } else if (route === 'document') {
      view.innerHTML = `<h1>지난달 지출결의</h1><p>${previous} · 주차비 환급 · 12,000원</p><button id="copy">사본 만들기</button>`;
      document.querySelector('#copy').onclick = () => navigate('edit');
    } else if (route === 'edit') {
      view.innerHTML = `<h1>지출결의 사본</h1><form id="expense"><label>문서 제목<input id="title" required value="${escape(state.title)}"></label><label>지출일<input id="date" type="date" required value="${state.date}"></label><label>금액<input id="amount" type="number" min="1" required value="${state.amount}"></label><label>양식<select id="template"><option value="refund">지출결의 · 환급신청서</option><option value="purchase">지출결의 · 구매신청서</option></select></label><button type="button" id="attach">증빙 첨부</button><p id="attachments" role="status">${escape(filesText())}</p><label>결재선<select id="approver" required><option value="">선택하세요</option><option value="team-finance">팀 검토 → 재무 검토</option><option value="team-only">팀 검토</option></select></label><button id="review" type="submit">상신 내용 확인</button><p id="validation" role="alert"></p></form>`;
      document.querySelector('#template').value = state.template;
      document.querySelector('#approver').value = state.approver;
      // React-like tracking: an own setter tracks direct assignments. Only a
      // native setter followed by an input event changes application state.
      for (const key of ['title', 'date', 'amount']) {
        const input = document.getElementById(key);
        const native = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
        let tracked = input.value;
        Object.defineProperty(input, 'value', { configurable: true, get() { return native.get.call(this); }, set(value) { tracked = String(value); native.set.call(this, value); } });
        input.addEventListener('input', () => {
          if (input.value !== tracked) { tracked = input.value; state[key] = tracked; }
        });
        input.addEventListener('blur', () => { input.value = state[key]; });
      }
      document.querySelector('#template').onchange = (event) => { state.template = event.target.value; };
      document.querySelector('#approver').onchange = (event) => { state.approver = event.target.value; };
      document.querySelector('#attach').onclick = () => { document.querySelector('#upload-status').textContent = filesText(); uploader.showModal(); };
      document.querySelector('#expense').onsubmit = (event) => {
        event.preventDefault();
        if (!state.files.length || state.template !== 'refund' || state.approver !== 'team-finance' || state.date === previous) {
          document.querySelector('#validation').textContent = '이번 지출일, 영수증, 환급 양식, 팀·재무 결재선을 확인하세요';
        } else navigate('review');
      };
    } else if (route === 'review') {
      view.innerHTML = `<h1>상신 확인</h1><dl><dt>제목</dt><dd>${escape(state.title)}</dd><dt>지출일</dt><dd>${state.date}</dd><dt>금액</dt><dd>${state.amount}원</dd><dt>양식</dt><dd>지출결의 · 환급신청서</dd><dt>첨부</dt><dd>${escape(filesText())}</dd><dt>결재선</dt><dd>팀 검토 → 재무 검토</dd></dl><button id="confirm">시험 상신 확정</button>`;
      document.querySelector('#confirm').onclick = () => { state.submitted = true; navigate('complete'); };
    } else if (route === 'complete' && state.submitted) {
      view.innerHTML = `<h1 id="complete">시험 상신 완료</h1><p>외부 전송 없음</p><output id="result">${escape(JSON.stringify(state))}</output>`;
    } else view.innerHTML = '<h1>업무 홈</h1><p>전자결재 메뉴에서 내 문서함을 여세요.</p>';
    view.removeAttribute('aria-busy');
  };
  window.addEventListener('hashchange', () => { view.setAttribute('aria-busy', 'true'); setTimeout(render, 80); });
  const take = (files) => { state.files = Array.from(files, ({ name, size, type }) => ({ name, size, type })); document.querySelector('#upload-status').textContent = filesText(); };
  document.querySelector('#receipt').onchange = (event) => take(event.target.files);
  document.querySelector('#drop-zone').ondragover = (event) => event.preventDefault();
  document.querySelector('#drop-zone').ondrop = (event) => { event.preventDefault(); take(event.dataTransfer.files); };
  document.querySelector('#upload-done').onclick = () => { uploader.close(); const label = document.querySelector('#attachments'); if (label) label.textContent = filesText(); };
  render();
})();
