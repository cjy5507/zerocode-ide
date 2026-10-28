# Web forms: one read, one fill per step

Read this when a page has a form to fill: `fields`, `fill`, a script step, dates, suggestion lists, dialogs, a read-only field a window picks. The text was moved here word for word from the former single-file guide.

Where a paragraph below says "above" or "below", it means the whole guide, which is now split over several files: the table in `SKILL.md` says which file holds what.

### Web forms: one read, one fill per step

A form is data, so fill it as data, not a picture per field:

1. `fields <pane-label>` — every field the page draws (below the fold and
   inside same-origin frames too), each with the handle to name it by, its
   words, what it holds, its choices and `*` for required, then the buttons
   beside them. A field nothing names is read by its table header, its term
   or the words just before it that the page draws (text it keeps hidden is no
   name), else by its own placeholder; one with none of these says
   `(이름 없음)`. Parts of one value side by side (a phone
   number in three boxes) are `<words> (1/3)`, `(2/3)`, … with their length, and a
   part with another after it says the short symbol the page draws between them
   (`(다음 칸 앞에 「@」)`) — cut the value there.
   A button that opens a list (`aria-haspopup="listbox"`) is a field too —
   `combobox`, named by the words beside it (never by the value it shows),
   holding what it shows, `(열림)` or `(닫힘)`, with the items of its list — and
   `fill` chooses an item by its words. Fields of one name are told apart by
   the title of the row, item or card each stands in (`<title>: <words>`),
   else numbered (`<words> #1`, `#2`, with `(같은 이름이라 번호만 붙임)`: the
   number is only an order). A group of radios is named by its title; the
   sentence between the title and the group is its `— 안내:`. A row of buttons
   that say whether they are pressed (`aria-pressed`: chips, a size to pick, the
   options to tick) is one field of kind `chips` — named by the row's title
   (`(제목 없음)` when the page gives it none: its options then say what it is),
   its options the buttons' words, its value the list of the ones pressed
   (`= ["Red"]`) — and its buttons are not listed again as buttons, and neither are
   the options of a group of radios the page draws with buttons. A number the page draws between
   two buttons, in a group of its own (`− 1 +`, a count of adults), is one field of kind `stepper` —
   named by the group's name, `= "1"` the number it shows — and its two buttons are not listed again
   as buttons: `fill` it with the number wanted (`"3"`) and the door presses the button that moves
   it until the number shows the asked one, whichever way the page draws them; past a limit it stops
   there and says `mismatch` with the number it shows. A button that stands beside one
   field — next to it in the nearest box that holds a field — stays a button and says whose it is,
   `(#handle 칸 곁)` (a "show" beside a password, an "apply" beside a code): press it when the
   page asks for what it does; a button beside no one field says nothing. A part
   the page names only by its own short words (an hour beside a minute, the boxes of
   a code) takes the caption of the group or row it stands in:
   `<caption> — <words>`. A required field whose value the door never reads (a
   password) is counted `값을 읽지 않는 필수`, not as empty. The count line's
   `필수 N` is what the page declares (`required`, `aria-required`); a page that
   marks required only with a `*` in the words declares none, so the line says
   `필수 표시 없음 — 이름에 *가 있는 칸 N, 그중 비어 있는 M` (and, beside some
   that are declared, `이름에만 *가 있는 칸`) — the stars are the page's words,
   not a declaration, and no other mark is read (a colour, a word such as
   "(필수)" or "required" in the text): a field the line does not count may
   still be wanted, so read the page's own words before you submit. In the buttons a button the page declares a submit
   (`type="submit"`) carries `(제출 단추)`; one without it may still send the form.
   A step with no field of its own (a review, a confirmation) still lists its
   buttons.
2. `fill <pane-label> --value-stdin` with one JSON object of handle → value
   for everything you know except the fields the read says are secret (`= (가림)`), in the page's order. Words for text and dates
   (`2026-11-03`), a choice's words for a select, radio or a dropdown the
   page draws itself, `true`/`false` for a checkbox, the list of the options'
   words for a group of chips (`{"<handle>": ["Red", "Blue"]}` — only the
   buttons whose state differs are pressed, the others are left as they are,
   `[]` lets go of all; one text such as `"Red, Blue"` is read as that list). The answer is a line
   per field — `✓` with what it holds now, `✗` and why, or `?` when what it reads back differs in
   its letters from the value given and the page's own state does not say whether it is the same
   value (a list button that shows its choice shortened while the item asked is not marked chosen,
   a date drawn without its year): `같은지는 볼 수 없음` and the text it shows — neither a difference
   nor a sameness is said, so look at what is shown (or read `fields`) before you judge, and do
   not write it again. A secret field — a password, a one-time code (the page's own mark, or a numeric box the page calls a
   code), a payment card's number, security code or expiry date (the page's own mark `cc-number`, `cc-csc`, `cc-exp…`, or the words it gives the
   field: card number, CVC, CVV, security code — and expiry, valid thru, MM/YY, 유효기간 when the field stands in the form that holds the card's number), and all the parts of a value when one of them is secret (the boxes of a number, one of which hides what is typed) — is read
   as `(가림)`, is never written by `fill` and its value never goes into a fill or an eval: you type it from stdin
   (`type <label> <handle> --value-stdin` — a secret select, such as a card's expiry month, takes the same road: the text from stdin names one of its options), or hand a code sent to the person to the person:
   a field inside a frame of the page's own origin is typed by its handle too, `#frame >> #field`; a frame of
   another origin or a sandboxed one is refused by name (`frame_sealed`), and a type held to a form that changed
   since the read is refused (`form_stale`) — the fields
   still empty and required, and a last line: `양식 그대로` or `양식 바뀜(fields로
   다시 읽기)`, then the buttons as they are now — `버튼: #next 「Next」 켜짐,
   #back 「Back」 꺼짐` (`꺼짐` is off; `숨김:` names one the page took away).
   A field an earlier value brings (the times a date loads, a box a choice
   turns on) is tried again inside the same call. The last line, each field's
   value and error, and the fields left are read after the page has stood
   still for 50 ms (as after a press); if it kept changing, the last line says
   `아직 바뀌는 중(fields로 다시 읽기)` — read `fields` before you press. A page
   that changes later than that is the next read's: a fill that waited ends with
   `※ … 그 뒤에 뜨는 오류는 못 봅니다 — 제출 전에 fields로 다시 읽으세요`, and
   it means it — an error a check shows only after a pause is not in the answer.
   Read `fields` again before you submit. The answer also says what stands new
   in the box around a field you wrote — `— 새로 뜬 글: 「…」`, the page's own
   words, never called an error: a check that writes its verdict beside the
   field and ties it to nothing is read this way (a number that only counts, a
   countdown, and the value the field itself shows are not new words) — marks a
   field the page calls invalid and says nothing of why with `⚠ aria-invalid —
   페이지가 이유를 말하지 않음` (only when no words but the field's own stand in
   its box: a sentence that stood beside it all along may be the reason, so
   then it says only `⚠ aria-invalid`), names what the page's alert and
   status regions said, beside no field, as `새로 뜬 알림: 「…」`, and says the
   text that stands new in the form beside no single field — the reason a page
   writes under a value made of several fields (an address and its domain) or
   under a group of buttons — as `새로 뜬 글(칸 밖): 「…」`, while the form is
   still the one you read.
3. Press the step's button by its handle (`click <pane-label> <handle>`) —
   a step's button that turns on once the fields are right is already `켜짐`
   on that last line, so press it from there with no `fields` between — and
   read `fields` again only when the page moved on to another step (`양식
   바뀜`). In a pane whose form you have read, a `click` answers more than its
   own sentence (which now says how the page settled): below it, `양식 그대로`
   or `양식 바뀜(fields로 다시 읽기)` with the buttons as they are now, and for
   a form that stayed `남은 칸:` with each field's error and the text that
   stands new beside it, `새로 뜬 알림`, `새로 뜬 글(칸 밖)`, and the same
   `※ … 못 봅니다` note. A
   "next" that does not move on says why there, in the page's words. A click in
   a pane whose form you never read is answered as it was.

A `fill` is held to the form your last `fields` read (or your last fill
left): when a field has gone, been renamed or added, or the page is on
another step, it writes nothing and answers `form_stale` — read the form
again, then fill. Fields your own values bring inside one fill are not
stale.

One step can also be one script: an `eval` that names `zerocode.` gets
`zerocode.fields()` (the same read) and `zerocode.fill({handle: value})`
(one pass of the same fill), so the script reads the step, fills it by the
words it read, checks what is left and presses the step's own button — one
round trip a step. Send it on stdin so the person's details stay off argv.
A script is a record of what it carries: write it after the read of the step
(`fields`, or the read the last script returned) and put in it the values of the
fields that read shows and does not say are secret (`= (가림)`) — never a secret
field's value, and nothing for a step you have not read; the secret fields are
typed with `type`. A script that ends by returning `zerocode.fields()` hands you
the read of the page it stopped on, to write the next one from:

```text
zerocode-browser eval <pane-label> --value-stdin <<'JS'
(() => {
  const read = zerocode.fields();
  const at = (words) => read.fields.find((f) => f.label.startsWith(words))?.handle;
  const step = zerocode.fill({ [at("Name")]: "Kim", [at("Arrival date")]: "2026-11-03", [at("I agree")]: true }, read);
  const next = read.actions.find((a) => a.label === "Next");
  if (!step.stale && step.results.every((r) => r.status === "set" || r.status === "same") && !step.left.length && next) {
    document.querySelector(next.handle).click();   // a step's own button — never a payment or a send
  }
  return { results: step.results, left: step.left, error: document.querySelector("[role=alert]")?.textContent, form: zerocode.fields() };
})()
JS
```

An eval is synchronous — what its `fill` reads back is the page in the same
instant it wrote, so a button the page turns on a moment later still reads off
there: press by the next read. A field the page loads later is the next call's, and
the script returns instead of pressing anything that pays, sends or cannot
be undone — that press is the person's word, asked first.

A date goes in as `2026-11-03` whatever the field shows: `fill` writes it
in the field's own format (its placeholder's `YYYY.MM.DD`) or, for a field
the page keeps from typing, pages the page's own calendar to the month and
presses the day. A field answered `no_option` (or `read_only`) will not
take the same `fill` again — do not resend it. When the answer carries a
calendar (`달력 「…」 넘김 … · 날짜 칸 …`), finish that one field by hand:
`click` the field to open it, `click` a pager until the heading shows the
month, `click` the day; then `fields` to check it.

A button that opens a dialog says the dialog's name in the buttons line — `#from 「Start」 (대화상자 「Stay dates」을 엶)` — and the days of a calendar the page draws are not listed as buttons
(the dialog's own buttons are: an arrow, the one that applies a range). Such a button may be a date picker: `fill` it with a date (`{"#from": "2026-11-20"}`) and the door opens the dialog, pages its
calendar to the month and presses the day. What a dialog shows is not the date until the page applies it, so the answer is `?`, not `✓`: then `fields` and press the dialog's button that turned on
(`Done`, `선택 완료`), and `fields` again to check what the page shows.

A text field the page keeps a list of suggestions beside (a box with the role of a list that is shut until something is typed) takes the item chosen from that list, not the
text typed: `fill` types the text and keeps the field — a page shuts the list when the field loses the focus —, waits in its passes for the list the page makes after the text,
and presses the item whose line is the words asked, so `fill` answers `✓` with what the page took. The fields after it in the bundle are written first. When no item is the words
asked it says `그 값의 선택지가 없음` with the items the list showed (`▸ …`), the text still typed: `click` one of them, or `fill` its words.

A read-only field is picked, not written, when the page fills it from a window a button opens (an address chosen from a search): the line says which button
opens it, `(직접 못 씀 — 열 단추: <handle> 「…」)` in a read and `(열 단추: <handle> 「…」)` in the answer of a `fill`. `click` that button, then `fields` again —
a window with a frame of its own may need a moment to load, so read again when nothing new is there. `fill` the words the value holds into the window's
search field (a field inside a frame is `<frame> >> <inner>`), then press the window's search button and read again for the results (the page may answer after a moment).
A button inside a frame is pressed with an `eval`, `String.raw` keeping a handle's backslashes: ``document.querySelector(String.raw`<frame>`).contentDocument.querySelector(String.raw`<inner>`).click()``.
Press the result whose words hold the value whole — `North Gate 9` is held by `Gate 9 · open` and not by `North Gate 90` — then `fields` to check that the field holds it.

A line `종류를 모르는 조작: …` names what a person can press there that the
read has no kind for — a box with its own list, a chip, a pointer-only
control: `click` it by its handle, then `fields` again; what opens is read
like the rest.

A field the page has switched off (`(꺼짐)`) or will not let you write
(`(직접 못 씀)`) says why after `— 안내:`, in the page's own words about it
(what it waits for, what turns it on) — act on that, not on a guess. A line
`끝까지 안 내린 스크롤 상자: …` names a box with more to read below what it
shows (terms to be read to the end before a box turns on): scroll that box to
its end with an `eval` — ``document.querySelector(String.raw`<handle>`).scrollTop = 1e9`` (`String.raw`
keeps a handle's backslashes; for a handle with ` >> ` in it, the part before is
the frame: `document.querySelector(String.raw`<frame>`).contentDocument.querySelector(String.raw`<inner>`)`)
— then `fields` again; it is said no more once read to its end.

Use the handles and words exactly as `fields` printed them; never guess a
selector. A password field is refused by `fill` (use `type … --value-stdin`),
a file field and a code sent to the person's phone are the person's turn
(`handoff`; for a code, `--ask-code --into '["browser","type","<pane-label>","<handle>","--workspace-revision","<revision>"]'`), and a field `fill` could not write says so — fall back to
`click` and a look for that one field only.
