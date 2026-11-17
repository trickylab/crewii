import { test } from 'node:test';
import assert from 'node:assert/strict';
import { placementIndex, newEvents, initialTabs } from '../assets/plugin/tabs.js';

test('placement follows last matching role or end', () => {
  const tabs = [{ title: 'ba-01' }, { title: 'reviewer-01' }, { title: 'qa-01' }, { title: 'reviewer-02' }];
  assert.equal(placementIndex(tabs.slice(0, 3), 'reviewer-02'), 2);
  assert.equal(placementIndex(tabs, 'reviewer-03'), 4);
  assert.equal(placementIndex(tabs, 'unknown-01'), 4);
  assert.equal(placementIndex([], 'ba-01'), 0);
});

test('placement excludes the tab being moved and tolerates missing titles', () => {
  const tabs = [{ title: 'reviewer-01' }, {}, { title: 'ba-01' }, { title: 'reviewer-02' }];
  assert.equal(placementIndex(tabs, 'reviewer-02'), 1);
  assert.equal(placementIndex(tabs, 'qa-01'), 4);
  assert.equal(placementIndex([{ title: 'qa-01' }, {}], 'qa-01'), 1);
});

test('new events filter and sort without mutation', () => {
  const events = [{ seq: 4 }, { seq: 1 }, { seq: 3 }, { seq: 2 }];
  assert.deepEqual(newEvents(events, 2), [{ seq: 3 }, { seq: 4 }]);
  assert.deepEqual(events.map(event => event.seq), [4, 1, 3, 2]);
  assert.deepEqual(newEvents(events, 4), []);
});

test('initial tabs put role leads first, then open ticket holders, deduplicated', () => {
  const config = { roles: [{ id: 'ba' }, { id: 'reviewer' }, { id: 'qa' }] };
  const state = {
    instances: {
      'ba-01': { sessionID: 'ses_ba' }, 'reviewer-01': { sessionID: 'ses_review' },
      'qa-01': {}, 'reviewer-02': { sessionID: 'ses_review2' }, 'reviewer-03': { sessionID: 'ses_review2' },
    },
    tickets: {
      'T-1': { holder: 'reviewer-02', status: 'open' },
      'T-2': { holder: 'ba-01', status: 'open' },
      'T-3': { holder: 'missing', status: 'open' },
      'T-4': { holder: 'qa-01', status: 'open' },
      'T-5': { holder: 'reviewer-03', status: 'open' },
      'T-6': { holder: 'reviewer-01', status: 'closed' },
    },
  };
  assert.deepEqual(initialTabs(config, state), ['ses_ba', 'ses_review', 'ses_review2']);
});
