#!/usr/bin/env node
/**
 * Drive the web client in Chrome over the DevTools protocol with TRUSTED input.
 * See context/live-testing.md.
 *
 * Trusted input is the point of this file. The browser MCP's `type_text`,
 * `human_type` and `press_key` do not produce what a real keyboard does:
 * `press_key` dispatches untrusted synthetic events that insert nothing, and
 * the typing tools insert text without any keydown at all. Every composer bug
 * so far has lived in the keydown -> beforeinput -> native-insertion path, so a
 * test driven by those tools passes while the bug stands.
 *
 *   cdp.mjs login <session.json>     put a session in localStorage and reload
 *   cdp.mjs goto <url>
 *   cdp.mjs reload
 *   cdp.mjs type <text> [delayMs]    one real keyDown/keyUp pair per character
 *   cdp.mjs key <Name>...            Enter Escape Tab Backspace Delete Arrow* Home End
 *   cdp.mjs click <css>              trusted click at the element's centre
 *   cdp.mjs clickxy <x> <y>
 *   cdp.mjs hover <css>              move the pointer onto the element
 *   cdp.mjs composer                 composer text, DOM caret, focus, Slate model
 *   cdp.mjs eval <js>                evaluate an expression, print the value
 *
 * Env: CDP_PORT (default: found from the running Chrome's command line),
 *      CDP_PAGE (substring of the tab URL to drive, default ":5199").
 */
import { readdirSync, readFileSync } from 'node:fs';

const PAGE_MATCH = process.env.CDP_PAGE ?? ':5199';

/** Named keys: [key, code, windowsVirtualKeyCode, text]. */
const NAMED_KEYS = {
  Enter: ['Enter', 'Enter', 13, '\r'],
  Escape: ['Escape', 'Escape', 27, ''],
  Tab: ['Tab', 'Tab', 9, ''],
  Backspace: ['Backspace', 'Backspace', 8, ''],
  Delete: ['Delete', 'Delete', 46, ''],
  ArrowUp: ['ArrowUp', 'ArrowUp', 38, ''],
  ArrowDown: ['ArrowDown', 'ArrowDown', 40, ''],
  ArrowLeft: ['ArrowLeft', 'ArrowLeft', 37, ''],
  ArrowRight: ['ArrowRight', 'ArrowRight', 39, ''],
  Home: ['Home', 'Home', 36, ''],
  End: ['End', 'End', 35, ''],
};

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** The browser MCP picks a random port per launch; read it off the process. */
const findPort = () => {
  if (process.env.CDP_PORT) return process.env.CDP_PORT;
  for (const pid of readdirSync('/proc').filter((p) => /^\d+$/.test(p))) {
    let cmdline;
    try {
      cmdline = readFileSync(`/proc/${pid}/cmdline`, 'utf8');
    } catch {
      continue;
    }
    const match = cmdline.match(/--remote-debugging-port=(\d+)/);
    if (match && match[1] !== '0') return match[1];
  }
  throw new Error('no Chrome with --remote-debugging-port found; set CDP_PORT');
};

const connect = async () => {
  const port = findPort();
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const tab = tabs.find((t) => t.type === 'page' && t.url.includes(PAGE_MATCH));
  if (!tab) throw new Error(`no tab whose URL contains "${PAGE_MATCH}" (port ${port})`);

  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let nextId = 0;
  const pending = new Map();
  ws.onmessage = (msg) => {
    const data = JSON.parse(msg.data);
    if (data.id && pending.has(data.id)) {
      pending.get(data.id)(data);
      pending.delete(data.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      nextId += 1;
      pending.set(nextId, resolve);
      ws.send(JSON.stringify({ id: nextId, method, params }));
    });
  return { send, close: () => ws.close() };
};

const evaluate = async (cdp, expression) => {
  const res = await cdp.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (res.result?.exceptionDetails) {
    throw new Error(res.result.exceptionDetails.exception?.description ?? 'evaluation failed');
  }
  return res.result?.result?.value;
};

const pressChar = async (cdp, ch) => {
  let code = '';
  if (/[a-z]/i.test(ch)) code = `Key${ch.toUpperCase()}`;
  else if (/[0-9]/.test(ch)) code = `Digit${ch}`;
  else if (ch === ' ') code = 'Space';
  const keyCode = /[a-z]/i.test(ch) ? ch.toUpperCase().charCodeAt(0) : ch.charCodeAt(0);
  const base = { key: ch, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', text: ch, unmodifiedText: ch, ...base });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};

const pressNamed = async (cdp, name) => {
  const def = NAMED_KEYS[name];
  if (!def) throw new Error(`unknown key "${name}" — one of ${Object.keys(NAMED_KEYS).join(' ')}`);
  const [key, code, keyCode, text] = def;
  const base = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
  // A key that produces text goes as keyDown with that text; one that does not
  // goes as rawKeyDown, which is what Chrome itself sends for it.
  await cdp.send('Input.dispatchKeyEvent', { type: text ? 'keyDown' : 'rawKeyDown', text, ...base });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
};

/**
 * Centre of the first match, read at the moment of use. Never reuse a position
 * across steps: banners like "Connecting to …" come and go and shift the whole
 * layout by a line, which turns a stale coordinate into a click on the wrong
 * thing — a miss that looks exactly like a broken button.
 */
const centreOf = async (cdp, selector) => {
  const pos = await evaluate(
    cdp,
    `(() => { const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null; const r = el.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`,
  );
  if (!pos) throw new Error(`no element matches ${selector}`);
  return pos;
};

const clickAt = async (cdp, x, y) => {
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
};

/**
 * Everything needed to tell the composer's failure modes apart. The DOM caret
 * and Slate's model selection can disagree, and which one is wrong is the whole
 * diagnosis — so both are reported, not just the text.
 */
const COMPOSER_STATE = `(() => {
  const ed = document.querySelector('[data-slate-editor]');
  if (!ed) return { error: 'no composer on screen' };
  const sel = getSelection();
  const active = document.activeElement;
  const describe = (node) => !node ? null
    : node.nodeType === 3 ? 'text:' + JSON.stringify(node.data)
    : node.nodeName + (node.className ? '.' + String(node.className).split(' ')[0] : '');
  const fiberKey = Object.keys(ed).find((k) => k.startsWith('__reactFiber'));
  let fiber = fiberKey && ed[fiberKey];
  while (fiber && !(fiber.memoizedProps && fiber.memoizedProps.editor && fiber.memoizedProps.editor.children)) {
    fiber = fiber.return;
  }
  const editor = fiber && fiber.memoizedProps.editor;
  return {
    text: ed.innerText.replace(/[\\u00a0\\ufeff\\n]/g, ''),
    focus: active === ed ? 'composer' : describe(active),
    domCaret: sel.rangeCount === 0 ? 'none'
      : describe(sel.anchorNode) + '@' + sel.anchorOffset + (ed.contains(sel.anchorNode) ? '' : ' (outside composer)'),
    model: editor ? {
      selection: editor.selection,
      doc: editor.children.map((block) => (block.children || []).map((n) => n.text !== undefined ? n.text : '<' + n.type + '>')),
    } : 'editor not found',
  };
})()`;

const main = async () => {
  const [command, ...args] = process.argv.slice(2);
  if (!command) {
    const header = readFileSync(new URL(import.meta.url), 'utf8').split('*/')[0];
    const usage = header.split('\n').filter((line) => line.startsWith(' *   cdp.mjs'));
    console.log(usage.map((line) => line.slice(3)).join('\n'));
    process.exit(2);
  }

  const cdp = await connect();
  try {
    switch (command) {
      case 'login': {
        const session = JSON.parse(readFileSync(args[0], 'utf8'));
        await evaluate(
          cdp,
          `(() => {
            localStorage.setItem('cinny_access_token', ${JSON.stringify(session.access_token)});
            localStorage.setItem('cinny_device_id', ${JSON.stringify(session.device_id)});
            localStorage.setItem('cinny_user_id', ${JSON.stringify(session.user_id)});
            localStorage.setItem('cinny_hs_base_url', ${JSON.stringify(session.hs_base_url)});
            location.reload(); return true; })()`,
        );
        console.log(`logged in as ${session.user_id}`);
        break;
      }
      case 'goto':
        await cdp.send('Page.navigate', { url: args[0] });
        break;
      case 'reload':
        await evaluate(cdp, 'location.reload(), true');
        break;
      case 'type': {
        const delay = Number(args[1] ?? 40);
        for (const ch of args[0] ?? '') {
          await pressChar(cdp, ch);
          await sleep(delay);
        }
        break;
      }
      case 'key':
        for (const name of args) {
          await pressNamed(cdp, name);
          await sleep(50);
        }
        break;
      case 'click': {
        const { x, y } = await centreOf(cdp, args[0]);
        await clickAt(cdp, x, y);
        break;
      }
      case 'clickxy':
        await clickAt(cdp, Number(args[0]), Number(args[1]));
        break;
      case 'hover': {
        const { x, y } = await centreOf(cdp, args[0]);
        // Two moves: react-aria's useHover needs a pointer that ENTERS the
        // element, and a single move to where the pointer already is enters
        // nothing.
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: x - 4, y: y - 4 });
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
        break;
      }
      case 'composer':
        console.log(JSON.stringify(await evaluate(cdp, COMPOSER_STATE), null, 2));
        break;
      case 'eval':
        console.log(JSON.stringify(await evaluate(cdp, args.join(' '))));
        break;
      default:
        throw new Error(`unknown command "${command}"`);
    }
  } finally {
    cdp.close();
  }
};

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
