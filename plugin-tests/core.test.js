import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptyState, instanceName, roleOf, parseTarget, route, openTicket, closeTicket, addEvent, senderTag, counterpart, rosterText } from '../assets/plugin/core.js';

const config = { division: 'engineering', roles: [{ id: 'ba', description: 'Business analyst' }, { id: 'reviewer', description: 'Reviews' }], limits: { instancesPerRole: 3, ticketsPerBa: 3 } };
const stateWith = (...names) => ({ ...emptyState(), instances: Object.fromEntries(names.map(name => [name, { role: roleOf(name), sessionID: `ses_${name}`, created: 0 }])) });
const ticket = (state, holder, now = 0) => openTicket(state, { requester: 'ses_ba', requesterName: 'ba-01', holder, text: 'hello', queued: false, now }).state;

test('instance names and target parsing', () => {
  assert.equal(instanceName('reviewer', 2), 'reviewer-02');
  assert.equal(roleOf('qa-10'), 'qa');
  assert.equal(roleOf('quality-check-01'), 'quality-check');
  assert.equal(roleOf('untitled'), 'untitled');
  assert.deepEqual(parseTarget('reviewer', config), { kind: 'role', role: 'reviewer' });
  assert.deepEqual(parseTarget('reviewer-02', config), { kind: 'instance', name: 'reviewer-02' });
  assert.deepEqual(parseTarget('ses_abc', config), { kind: 'session', sessionID: 'ses_abc' });
  assert.match(parseTarget('marketing/ba', config).message, /cross-division/i);
  assert.match(parseTarget('nope', config).message, /unknown role/i);
  assert.match(parseTarget('nope-01', config).message, /unknown role/i);
});

test('routing uses lowest free instance, creates gaps, queues oldest', () => {
  let state = stateWith('reviewer-01');
  assert.deepEqual(route(state, config, 'reviewer'), { instance: 'reviewer-01' });
  state = ticket(state, 'reviewer-01', 20);
  assert.deepEqual(route(state, config, 'reviewer'), { create: 'reviewer-02' });
  state.instances['reviewer-02'] = { role: 'reviewer' };
  assert.deepEqual(route(state, config, 'reviewer'), { instance: 'reviewer-02' });
  state.instances['reviewer-03'] = { role: 'reviewer' };
  state = ticket(ticket(state, 'reviewer-02', 10), 'reviewer-03', 30);
  assert.deepEqual(route(state, config, 'reviewer'), { backlog: true });
  delete state.instances['reviewer-02'];
  assert.deepEqual(route(state, config, 'reviewer'), { create: 'reviewer-02' });
  assert.deepEqual(route(emptyState(), config, 'reviewer'), { create: 'reviewer-01' });
});

test('BA capacity counts only open tickets', () => {
  let state = stateWith('ba-01');
  state = ticket(ticket(state, 'ba-01'), 'ba-01');
  assert.deepEqual(route(state, config, 'ba'), { instance: 'ba-01' });
  state = ticket(state, 'ba-01');
  assert.deepEqual(route(state, config, 'ba'), { create: 'ba-02' });
  state = closeTicket(state, 'T-1', 4);
  assert.deepEqual(route(state, config, 'ba'), { instance: 'ba-01' });
});

test('tickets and events are immutable, numbered and capped', () => {
  const original = emptyState();
  const first = openTicket(original, { requester: 'user', requesterName: 'user', holder: 'ba-01', text: `first\n${'x'.repeat(80)}`, queued: true, now: 5 });
  assert.equal(first.ticket.id, 'T-1');
  assert.equal(first.ticket.title.length, 60);
  assert.doesNotMatch(first.ticket.title, /\n/);
  assert.equal(first.ticket.queued, undefined);
  assert.equal(first.ticket.created, 5);
  assert.equal(first.ticket.closed, null);
  assert.deepEqual(original, emptyState());
  const second = openTicket(first.state, { requester: 'user', requesterName: 'user', holder: 'ba-01', text: 'ok', queued: false, now: 6 });
  assert.equal(second.ticket.id, 'T-2');
  const closed = closeTicket(second.state, 'T-1', 7);
  assert.equal(closed.tickets['T-1'].status, 'closed');
  assert.equal(closed.tickets['T-1'].closed, 7);
  assert.equal(second.state.tickets['T-1'].status, 'open');
  assert.throws(() => closeTicket(closed, 'T-99', 0), /unknown ticket/i);
  let events = original;
  for (let i = 0; i < 101; i++) events = addEvent(events, { type: 'assigned', instance: 'ba-01', seq: -1, created: -1 }, i);
  assert.equal(events.seq, 101);
  assert.equal(events.events.length, 100);
  assert.equal(events.events[0].seq, 2);
  assert.equal(events.events[99].created, 100);
  assert.deepEqual(original.events, []);
});

test('sender tag, counterparts and roster', () => {
  const state = ticket(stateWith('ba-01', 'reviewer-01'), 'reviewer-01');
  assert.equal(senderTag({ name: 'ba-01', sessionID: 'ses_x', ticket: 'T-3' }), '[from ba-01 ses_x ticket T-3]');
  assert.equal(counterpart(state, 'T-1', 'ba-01'), 'reviewer-01');
  assert.equal(counterpart(state, 'T-1', 'reviewer-01'), 'ses_ba');
  assert.throws(() => counterpart(state, 'missing', 'ba-01'), /unknown ticket/i);
  const roster = rosterText(config, state);
  for (const text of ['ba', 'Business analyst', 'reviewer', 'Reviews', 'crew_send', 'reviewer-01']) assert.ok(roster.includes(text));
});

test('ticket title truncates by Unicode code points', () => {
  const { ticket: result } = openTicket(emptyState(), { requester: 'user', requesterName: 'user', holder: 'ba-01', text: `${'a'.repeat(59)}😀z`, queued: false, now: 1 });
  assert.equal(result.title, `${'a'.repeat(59)}😀`);
});

test('double close preserves the original state and timestamp', () => {
  const closed = closeTicket(ticket(emptyState(), 'ba-01'), 'T-1', 7);
  assert.strictEqual(closeTicket(closed, 'T-1', 9), closed);
  assert.equal(closed.tickets['T-1'].closed, 7);
});

test('BA at instance limit queues behind oldest ticket, resolving ties by insertion order', () => {
  const limits = { ...config, limits: { instancesPerRole: 1, ticketsPerBa: 2 } };
  let state = stateWith('ba-01');
  state = ticket(ticket(state, 'ba-01', 3), 'ba-01', 4);
  assert.deepEqual(route(state, limits, 'ba'), { backlog: true });
  let reviewers = stateWith('reviewer-01', 'reviewer-02', 'reviewer-03');
  reviewers = ticket(ticket(ticket(reviewers, 'reviewer-02', 1), 'reviewer-01', 1), 'reviewer-03', 2);
  assert.deepEqual(route(reviewers, config, 'reviewer'), { backlog: true });
});

test('roster explains addressing and omits empty instance list', () => {
  const text = rosterText(config, emptyState());
  for (const part of ['crew_send', 'role id', 'free instance', 'instance name', 'reviewer-02', 'session id', 'ticket', '[from', 'final: true']) assert.ok(text.includes(part), part);
  assert.ok(!text.includes('Instances:'));
});
