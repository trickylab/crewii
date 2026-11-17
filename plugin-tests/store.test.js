import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { createStore } from '../assets/plugin/store.js';
import { emptyState } from '../assets/plugin/core.js';

function setup(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'crewii test '));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const file = path.join(dir, 'nested dir', 'state.json');
  return { file, store: createStore(file, { fs, now: () => 42 }) };
}

test('missing state is empty; writes atomically in directory with spaces', async t => {
  const { file, store } = setup(t);
  assert.deepEqual(store.read(), emptyState());
  const result = await store.update(state => ({ ...state, seq: 2 }));
  assert.equal(result.seq, 2);
  assert.deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), result);
  assert.equal(fs.existsSync(`${file}.tmp`), false);
  assert.deepEqual(store.read(), result);
});

test('corrupt state is backed up and reset', t => {
  const { file, store } = setup(t);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, '{bad');
  assert.deepEqual(store.read(), emptyState());
  assert.equal(fs.readFileSync(`${file}.corrupt-42`, 'utf8'), '{bad');
  assert.equal(fs.existsSync(file), false);
});

test('corrupt ticket ids advance the recovered sequence', t => {
  const { file, store } = setup(t);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, '{ T-8 T-21 T-9007199254740992');
  assert.equal(store.read().nextTicket, 22);
  assert.equal(store.read().nextTicket, 22);
});

test('concurrent and async updates serialize; failed update does not poison queue', async t => {
  const { store } = setup(t);
  const updates = await Promise.all([
    store.update(async state => ({ ...state, seq: state.seq + 1 })),
    store.update(state => ({ ...state, seq: state.seq + 1 })),
  ]);
  assert.deepEqual(updates.map(s => s.seq), [1, 2]);
  await assert.rejects(store.update(() => { throw new Error('no'); }), /no/);
  assert.equal((await store.update(state => ({ ...state, seq: state.seq + 1 }))).seq, 3);
});

test('read errors other than missing and write errors propagate', async t => {
  const { file } = setup(t);
  const error = Object.assign(new Error('denied'), { code: 'EACCES' });
  const readStore = createStore(file, { fs: { ...fs, readFileSync() { throw error; } }, now: () => 1 });
  assert.throws(() => readStore.read(), /denied/);
  const writeStore = createStore(file, { fs: { ...fs, writeFileSync() { throw error; } }, now: () => 1 });
  await assert.rejects(writeStore.update(s => s), /denied/);
});

test('invalid JSON state shapes are backed up', async t => {
  const { file, store } = setup(t);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  for (const value of [null, {}, [], { ...emptyState(), seq: -1 }, { ...emptyState(), nextTicket: 0 }, { ...emptyState(), seq: Number.MAX_SAFE_INTEGER + 1 }, { ...emptyState(), instances: [] }, { ...emptyState(), tickets: 'bad' }, { ...emptyState(), events: {} }]) {
    fs.writeFileSync(file, JSON.stringify(value));
    assert.deepEqual(store.read(), emptyState());
    assert.equal(fs.readFileSync(`${file}.corrupt-42`, 'utf8'), JSON.stringify(value));
  }
});

test('newer state is preserved and writes are private', async t => {
  const { file, store } = setup(t);
  await store.update(s => s);
  assert.equal(fs.statSync(file).mode & 0o777, 0o600);
  fs.writeFileSync(file, JSON.stringify({ ...emptyState(), version: 2 }));
  assert.throws(() => store.read(), /state file version 2 is newer than this crewii; upgrade crewii/);
  assert.equal(JSON.parse(fs.readFileSync(file)).version, 2);
});

test('updates across store instances for the same file serialize', async t => {
  const { file, store } = setup(t);
  const second = createStore(file, { fs, now: () => 42 });
  const results = await Promise.all([
    store.update(async state => ({ ...state, seq: state.seq + 1 })),
    second.update(state => ({ ...state, seq: state.seq + 1 })),
  ]);
  assert.deepEqual(results.map(state => state.seq), [1, 2]);
  assert.equal(store.read().seq, 2);
});

test('failed atomic rename preserves original file', async t => {
  const { file, store } = setup(t);
  await store.update(state => ({ ...state, seq: 1 }));
  const failing = createStore(file, { fs: { ...fs, renameSync() { throw new Error('rename failed'); } }, now: () => 42 });
  await assert.rejects(failing.update(state => ({ ...state, seq: 2 })), /rename failed/);
  assert.equal(JSON.parse(fs.readFileSync(file, 'utf8')).seq, 1);
  assert.equal((await store.update(state => ({ ...state, seq: state.seq + 1 }))).seq, 2);
});
