import { test, mock } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import fs from 'node:fs';
import plugin, { createTui } from '../assets/plugin/tui.js';

function fixture(enabled = true, shape = 'sessionID') {
  const dir = mkdtempSync(join(tmpdir(), 'crewii tui '));
  const statePath = join(dir, 'state.json');
  const configPath = join(dir, 'config.json');
  const config = { division: 'engineering', roles: [{ id: 'ba' }, { id: 'reviewer' }, { id: 'qa' }] };
  const state = { version: 1, seq: 0, instances: {
    'ba-01': { sessionID: 'ses_ba' }, 'reviewer-01': { sessionID: 'ses_r1' },
    'reviewer-02': { sessionID: 'ses_r2' }, 'qa-01': { sessionID: 'ses_qa' },
  }, tickets: {}, events: [] };
  writeFileSync(configPath, JSON.stringify(config));
  const save = () => writeFileSync(statePath, JSON.stringify(state));
  save();
  const tabs = [], moves = [], toasts = [], focuses = [];
  let tick, interval, cleared, command, selection, options;
  const ctx = { ui: {
    tabs: { enabled: () => enabled, list: () => tabs.map(id => ({ [shape]: id })),
      open: id => { if (!tabs.includes(id)) tabs.push(id); },
      move: (id, index) => { moves.push([id, index]); tabs.splice(tabs.indexOf(id), 1); tabs.splice(index, 0, id); },
      focus: id => focuses.push(id), close: id => tabs.splice(tabs.indexOf(id), 1) },
    toast: { show: ({ message }) => toasts.push(message) },
    dialog: { select: async value => { options = value; return selection; } },
  }, keymap: { layer: fn => { command = fn().commands[0]; } } };
  const tui = createTui({ fs, statePath, configPath,
    setInterval: (fn, ms) => { tick = fn; interval = ms; return 42; },
    clearInterval: id => { cleared = id; } });
  return { state, statePath, configPath, save, ctx, tui, tabs, moves, toasts, focuses,
    tick: () => tick(), interval: () => interval, cleared: () => cleared,
    command: () => command, select: value => { selection = value; }, options: () => options,
    dispose: () => rmSync(dir, { recursive: true, force: true }) };
}

test('tabs off toast once without polling', () => {
  const f = fixture(false);
  try { f.tui.setup(f.ctx); assert.deepEqual(f.toasts, ['crewii: session tabs are off']); assert.equal(f.interval(), undefined); } finally { f.dispose(); }
});

test('initial tabs follow role order then open ticket holders without focus', () => {
  const f = fixture();
  try {
    f.state.tickets['T-1'] = { holder: 'reviewer-02', status: 'open' }; f.save();
    f.tui.setup(f.ctx);
    assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_qa', 'ses_r2']);
    assert.deepEqual(f.focuses, []); assert.equal(f.interval(), 1000);
  } finally { f.dispose(); }
});

test('assignment opens then places after same role using either tab shape', () => {
  for (const shape of ['sessionID', 'id']) {
    const f = fixture(true, shape);
    try {
      f.tui.setup(f.ctx); f.moves.length = 0;
      f.state.seq = 1; f.state.events.push({ seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'ba-01' }); f.save(); f.tick();
      assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_r2', 'ses_qa']);
      assert.deepEqual(f.moves, [['ses_r2', 2]]); assert.deepEqual(f.toasts, ['reviewer-02: T-1 from ba-01']);
      f.tick(); assert.equal(f.toasts.length, 1);
    } finally { f.dispose(); }
  }
});

test('already open tabs stay in place; closed tabs reopen only on assignment', () => {
  const f = fixture();
  try {
    f.tui.setup(f.ctx); f.moves.length = 0;
    f.state.seq = 1; f.state.events.push({ seq: 1, type: 'assigned', instance: 'reviewer-01', ticket: 'T-1', from: 'ba-01' }); f.save(); f.tick();
    assert.deepEqual(f.moves, []); f.ctx.ui.tabs.close('ses_r1'); f.tick(); assert.equal(f.tabs.includes('ses_r1'), false);
    f.state.seq = 2; f.state.events.push({ seq: 2, type: 'assigned', instance: 'reviewer-01', ticket: 'T-2', from: 'ba-01' }); f.save(); f.tick();
    assert.equal(f.tabs.includes('ses_r1'), true); assert.equal(f.moves.length, 1);
  } finally { f.dispose(); }
});

test('missing, corrupt, and wrong-shaped state are ignored without altering files', () => {
  const f = fixture();
  try {
    f.tui.setup(f.ctx); rmSync(f.statePath); assert.doesNotThrow(f.tick);
    for (const bad of ['{', 'null', '{}', '{"seq":1,"instances":[],"tickets":{},"events":[]}',
      JSON.stringify({ seq: 1, instances: { broken: { sessionID: 7 } }, tickets: {}, events: [] }),
      JSON.stringify({ seq: 1, instances: {}, tickets: { broken: {} }, events: [] })]) {
      writeFileSync(f.statePath, bad); assert.doesNotThrow(f.tick); assert.equal(fs.readFileSync(f.statePath, 'utf8'), bad);
    }
    assert.deepEqual(f.toasts, []);
    f.state.seq = 1; f.state.events.push({ seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'ba-01' }); f.save(); f.tick();
    assert.equal(f.tabs.includes('ses_r2'), true);
  } finally { f.dispose(); }
});

test('crew command lists current open tickets and focuses selection', async () => {
  const f = fixture();
  try {
    f.tui.setup(f.ctx); f.state.tickets['T-1'] = { holder: 'reviewer-02', status: 'open' }; f.save();
    const cmd = f.command(); assert.equal(cmd.id, 'crewii.crew'); assert.equal(cmd.palette, true); assert.equal(cmd.slash.name, 'crew');
    f.select('ses_r2'); await cmd.run(); assert.deepEqual(f.focuses, ['ses_r2']);
    assert.ok(f.options().options.some(o => o.title === 'reviewer-02 · T-1' && o.value === 'ses_r2'));
    assert.ok(f.options().options.some(o => o.title === 'ba-01 · idle'));
    f.select(undefined); await cmd.run(); assert.deepEqual(f.focuses, ['ses_r2']);
  } finally { f.dispose(); }
});

test('crew dialog lists backlog after instances without focusing it', async () => {
  const f = fixture();
  try {
    f.state.tickets['T-8'] = { holder: '', role: 'reviewer', status: 'backlog', created: Date.now() - 120_000 }; f.save();
    f.tui.setup(f.ctx); f.select(''); await f.command().run();
    assert.match(f.options().options.at(-1).title, /^T-8 · reviewer · waiting 2m$/);
    assert.deepEqual(f.focuses, []);
  } finally { f.dispose(); }
});

test('tui sorts urgent first', async () => {
  const f = fixture();
  try {
    f.state.tickets['T-1'] = { holder: '', role: 'reviewer', status: 'backlog', created: 1 };
    f.state.tickets['T-2'] = { holder: '', role: 'reviewer', status: 'backlog', created: 2, urgent: true };
    f.state.tickets['T-3'] = { holder: '', role: 'reviewer', status: 'backlog', created: 3, urgent: true, backlogAt: -10 };
    f.save(); f.tui.setup(f.ctx); await f.command().run();
    assert.deepEqual(f.options().options.slice(-3).map(o => o.title.split(' · ')[0]), ['T-3', 'T-2', 'T-1']);
    assert.match(f.options().options.at(-3).title, /^T-3 · reviewer urgent · waiting/);
  } finally { f.dispose(); }
});

test('cleanup clears interval and default plugin has required identity', () => {
  const f = fixture();
  try { assert.equal(plugin.id, 'crewii.tui'); assert.equal(typeof plugin.setup, 'function'); f.tui.setup(f.ctx)(); assert.equal(f.cleared(), 42); } finally { f.dispose(); }
});

test('default plugin reads config and division state relative to plugin directory', () => {
  const f = fixture();
  const original = fs.readFileSync;
  const paths = [];
  const spy = mock.method(fs, 'readFileSync', (name, ...args) => {
    if (typeof name !== 'string') return original(name, ...args);
    paths.push(name);
    if (name.endsWith('/assets/plugin/config.json')) return JSON.stringify({ division: 'engineering', roles: [] });
    if (name.endsWith('/state/engineering.json')) return JSON.stringify({ seq: 0, instances: {}, tickets: {}, events: [] });
    return original(name, ...args);
  });
  try { plugin.setup(f.ctx)(); assert.equal(paths.length, 2); assert.ok(paths[1].endsWith('/state/engineering.json')); }
  finally { spy.mock.restore(); f.dispose(); }
});

test('recovers when state appears after setup and ignores other events', async () => {
  const f = fixture();
  try {
    rmSync(f.statePath); const cleanup = f.tui.setup(f.ctx);
    await f.command().run(); assert.deepEqual(f.focuses, []);
    f.save(); f.tick(); assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_qa']); f.moves.length = 0;
    f.state.seq = 2; f.state.events.push({ seq: 1, type: 'closed' }, { seq: 2, type: 'assigned', instance: 'missing', ticket: 'T-1', from: 'user' }); f.save(); f.tick();
    assert.deepEqual(f.moves, []); assert.deepEqual(f.toasts, ['crewii: no crew state yet']); cleanup();
  } finally { f.dispose(); }
});

test('reserved null session does not hide other instances', () => {
  const f = fixture();
  try {
    f.state.instances['reviewer-02'].sessionID = null; f.save(); f.tui.setup(f.ctx);
    assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_qa']);
  } finally { f.dispose(); }
});

test('event UI errors do not escape or replay and later events still run', () => {
  for (const failure of ['list', 'open', 'move', 'toast']) {
    const f = fixture();
    try {
      f.tui.setup(f.ctx);
      const target = failure === 'toast' ? f.ctx.ui.toast : f.ctx.ui.tabs;
      const method = failure === 'toast' ? 'show' : failure;
      const original = target[method]; let failed = false;
      target[method] = (...args) => { if (!failed) { failed = true; throw Error('UI failure'); } return original(...args); };
      f.state.seq = 2; f.state.events.push(
        { seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'ba-01' },
        { seq: 2, type: 'assigned', instance: 'qa-01', ticket: 'T-2', from: 'ba-01' });
      f.save(); assert.doesNotThrow(f.tick); assert.ok(f.toasts.includes('qa-01: T-2 from ba-01'));
      const count = f.toasts.length; f.tick(); assert.equal(f.toasts.length, count);
    } finally { f.dispose(); }
  }
});

test('failed keymap registration never starts a timer', () => {
  const f = fixture();
  try {
    f.ctx.keymap.layer = () => { throw Error('keymap'); };
    assert.throws(() => f.tui.setup(f.ctx), /keymap/); assert.equal(f.interval(), undefined);
  } finally { f.dispose(); }
});

test('state sequence reset allows new assignments to be processed', () => {
  const f = fixture();
  try {
    f.state.seq = 10; f.save(); f.tui.setup(f.ctx);
    f.state.seq = 1; f.state.events = [{ seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'user' }]; f.save(); f.tick();
    assert.equal(f.tabs.includes('ses_r2'), false);
    f.state.seq = 2; f.state.events.push({ seq: 2, type: 'assigned', instance: 'reviewer-02', ticket: 'T-2', from: 'user' }); f.save(); f.tick();
    assert.ok(f.tabs.includes('ses_r2')); assert.deepEqual(f.toasts, ['reviewer-02: T-2 from user']);
  } finally { f.dispose(); }
});

test('crew lists all open tickets and announces missing state', async () => {
  const f = fixture();
  try {
    f.state.tickets = { 'T-1': { holder: 'reviewer-02', status: 'open' }, 'T-2': { holder: 'reviewer-02', status: 'closed' }, 'T-3': { holder: 'reviewer-02', status: 'open' } };
    f.save(); f.tui.setup(f.ctx); await f.command().run();
    assert.ok(f.options().options.some(option => option.title === 'reviewer-02 · T-1, T-3'));
    rmSync(f.statePath); await f.command().run(); assert.deepEqual(f.toasts, ['crewii: no crew state yet']);
  } finally { f.dispose(); }
});

test('invalid division rejects setup before resolving state path', () => {
  for (const division of ['../escape', '', null, 'name.with.dots', 'bad/name']) {
    const f = fixture();
    try {
      writeFileSync(f.configPath, JSON.stringify({ division, roles: [] }));
      f.tui.setup(f.ctx); assert.equal(f.interval(), undefined); assert.equal(f.tabs.length, 0);
      assert.equal(f.toasts.length, 1); assert.match(f.toasts[0], /invalid.*config/i);
    } finally { f.dispose(); }
  }
  const f = fixture();
  try {
    let pathUsed = false;
    writeFileSync(f.configPath, JSON.stringify({ division: '../escape', roles: [] }));
    const tui = createTui({ fs, configPath: f.configPath, statePath: () => { pathUsed = true; }, setInterval: () => {}, clearInterval: () => {} });
    tui.setup(f.ctx); assert.equal(pathUsed, false);
  } finally { f.dispose(); }
});

test('late shuffled leads are ordered one move per poll, then left alone', () => {
  const f = fixture();
  try {
    f.state.instances = {}; f.save(); f.tabs.push('ses_stale1', 'ses_stale2'); f.tui.setup(f.ctx);
    f.state.instances = { 'ba-01': { sessionID: 'ses_ba' }, 'reviewer-01': { sessionID: 'ses_r1' } };
    f.tabs.push('ses_r1', 'ses_ba'); f.save(); f.tick(); assert.deepEqual(f.moves, []);
    f.state.instances['qa-01'] = { sessionID: 'ses_qa' };
    f.tabs.unshift('ses_qa'); f.save(); f.tick();
    assert.deepEqual(f.moves, [['ses_ba', 0]]);
    f.tick(); assert.deepEqual(f.moves, [['ses_ba', 0], ['ses_r1', 1]]);
    f.tick(); assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_qa', 'ses_stale1', 'ses_stale2']);
    f.tick(); f.ctx.ui.tabs.close('ses_ba'); f.tick();
    assert.deepEqual(f.tabs, ['ses_r1', 'ses_qa', 'ses_stale1', 'ses_stale2']); assert.equal(f.moves.length, 2);
  } finally { f.dispose(); }
});

test('lead ordering retries a failed move', () => {
  const f = fixture();
  try {
    f.state.instances['qa-01'].sessionID = null; f.save(); f.tui.setup(f.ctx);
    f.state.instances['qa-01'].sessionID = 'ses_qa'; f.save();
    f.tabs.unshift('ses_stale');
    const move = f.ctx.ui.tabs.move;
    f.ctx.ui.tabs.move = (id, index) => { if (id === 'ses_qa') throw Error('move'); move(id, index); };
    f.tick(); f.tick(); f.tick(); assert.ok(f.tabs.includes('ses_qa')); assert.deepEqual(f.moves, [['ses_ba', 0], ['ses_r1', 1]]);
    f.ctx.ui.tabs.move = move; f.tick(); assert.deepEqual(f.moves, [['ses_ba', 0], ['ses_r1', 1], ['ses_qa', 2]]);
  } finally { f.dispose(); }
});

test('retries restoration reshuffle then stops once verified', () => {
  const f = fixture();
  try {
    f.tabs.push('ses_qa', 'ses_r1', 'ses_ba'); f.tui.setup(f.ctx);
    assert.deepEqual(f.tabs, ['ses_ba', 'ses_qa', 'ses_r1']); assert.equal(f.moves.length, 1);
    f.tabs.splice(0, 3, 'ses_qa', 'ses_ba', 'ses_r1'); f.tick();
    assert.deepEqual(f.tabs, ['ses_ba', 'ses_qa', 'ses_r1']); assert.equal(f.moves.length, 2);
    f.tick(); assert.deepEqual(f.tabs, ['ses_ba', 'ses_r1', 'ses_qa']);
    f.tick(); f.tabs.splice(0, 3, 'ses_qa', 'ses_ba', 'ses_r1'); f.tick();
    assert.deepEqual(f.tabs, ['ses_qa', 'ses_ba', 'ses_r1']); assert.equal(f.moves.length, 3);
  } finally { f.dispose(); }
});

test('ordering stops after thirty unsuccessful attempts', () => {
  const f = fixture();
  try {
    let attempts = 0;
    f.tabs.push('ses_qa', 'ses_r1', 'ses_ba');
    f.ctx.ui.tabs.move = () => { attempts++; };
    f.tui.setup(f.ctx);
    for (let i = 0; i < 35; i++) f.tick();
    assert.equal(attempts, 30);
  } finally { f.dispose(); }
});

test('multiple new assignments use at most one move per poll without replay', () => {
  const f = fixture();
  try {
    f.tui.setup(f.ctx); f.tick(); f.moves.length = 0;
    f.state.instances['reviewer-03'] = { sessionID: 'ses_r3' };
    f.state.seq = 2; f.state.events = [
      { seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'ba-01' },
      { seq: 2, type: 'assigned', instance: 'reviewer-03', ticket: 'T-2', from: 'ba-01' },
    ];
    f.save(); f.tick(); assert.deepEqual(f.moves, [['ses_r2', 2]]); assert.equal(f.toasts.length, 1);
    f.tick(); assert.deepEqual(f.moves, [['ses_r2', 2], ['ses_r3', 3]]); assert.equal(f.toasts.length, 2);
    f.tick(); assert.equal(f.toasts.length, 2);
  } finally { f.dispose(); }
});

test('lead move and assignment placement are never issued on the same poll', () => {
  const f = fixture();
  try {
    f.tabs.push('ses_qa', 'ses_r1', 'ses_ba'); f.tui.setup(f.ctx); f.moves.length = 0;
    f.state.seq = 1; f.state.events = [{ seq: 1, type: 'assigned', instance: 'reviewer-02', ticket: 'T-1', from: 'ba-01' }]; f.save();
    f.tick(); assert.equal(f.moves.length, 1); assert.equal(f.tabs.includes('ses_r2'), false);
    f.tick(); assert.equal(f.moves.length, 2); assert.ok(f.tabs.includes('ses_r2')); assert.equal(f.toasts.length, 1);
  } finally { f.dispose(); }
});

test('restored tabs of removed crew sessions are closed; other tabs stay', () => {
  const f = fixture();
  try {
    const titles = { ses_old_ba: 'ba-01', ses_old_r: 'reviewer-02', ses_user: 'notes', ses_other: 'deploy-01' };
    f.ctx.ui.tabs.list = () => f.tabs.map(id => ({ sessionID: id, title: titles[id] ?? id }));
    f.tabs.push('ses_old_ba', 'ses_user', 'ses_old_r', 'ses_other');
    f.tui.setup(f.ctx);
    for (const id of ['ses_old_ba', 'ses_old_r']) assert.equal(f.tabs.includes(id), false);
    for (const id of ['ses_user', 'ses_other', 'ses_ba', 'ses_r1', 'ses_qa']) assert.ok(f.tabs.includes(id));
    const close = f.ctx.ui.tabs.close; f.ctx.ui.tabs.close = () => { throw Error('close'); };
    f.tabs.push('ses_old_ba'); f.tick(); assert.ok(f.tabs.includes('ses_old_ba'));
    f.ctx.ui.tabs.close = close; f.tick(); assert.equal(f.tabs.includes('ses_old_ba'), false);
  } finally { f.dispose(); }
});
