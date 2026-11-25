// C6 run f128741b: selected events in their original order and timestamps, with content redacted.
import { test, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { readFile, rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
let built, ThreadModel, events;
before(async () => {
  built = await buildHost(); ({ ThreadModel } = await built.load('threads'));
  events = JSON.parse(await readFile(new URL('./fixtures/c6-turns.json', import.meta.url), 'utf8'));
});
after(async () => { if (built) await rm(built.dir, { recursive: true, force: true }); });
const thread = () => new ThreadModel({ id:'c6', createdAt: 1795508819097, model:'claude', mode:'confirm' });
const apply = (t, from, to) => events.filter(e => e.seq >= from && e.seq <= to).forEach(e => t.applyEvent(e));
const seconds = r => (Date.parse(r.completedAt) - Date.parse(r.startedAt)) / 1000;

test('C6 earlier stopped turn remains settled when the next real turn starts', () => {
  const t = thread(); apply(t, 2, 54);
  const before = t.projection().runs.map(r => [r.status, r.startedAt, r.completedAt]);
  apply(t, 56, 56);
  assert.deepEqual(t.projection().runs.slice(0, before.length).map(r => [r.status, r.startedAt, r.completedAt]), before);
  apply(t, 57, 93);
  assert.deepEqual(t.projection().runs.slice(0, before.length).map(r => [r.status, r.startedAt, r.completedAt]), before);
});

test('C6 short response preserves its local send time through the engine echo and restart', () => {
  const t=thread(); const user=events.find(e=>e.seq===5);
  const sent=events.find(e=>e.seq===17).time-38000;
  t.sendLocal('local-1','reply: hi',sent); t.receipt(user.data.id,'local-1');
  apply(t,2,17);
  const start=t.promptTime('local-1');
  assert.equal(start,sent,'the persisted prompt time is the time of send, not delayed engine startup');
  assert.equal(seconds(t.projection().runs[0]),38);
  apply(t,18,93);
  assert.equal(seconds(t.projection().runs[0]),38, 'later turns leave the short response unchanged');
  const restored=thread();restored.restorePromptTimes({[user.data.id]:start});apply(restored,2,93);
  assert.equal(seconds(restored.projection().runs[0]),38);
  assert.deepEqual(restored.projection().runs.map(r => [r.status, r.startedAt, r.completedAt]), t.projection().runs.map(r => [r.status, r.startedAt, r.completedAt]));
});

test('the durable C6 turn/end corrects an earlier idle-status guess', () => {
  const t = thread();
  apply(t, 2, 12);
  const end = events.find(e => e.seq === 17).time;
  t.setRunning(false, end + 5000);
  apply(t, 17, 17);
  assert.equal(Date.parse(t.projection().runs[0].completedAt), end);
});

test('an idle-status guess can be reopened if the engine is still running', () => {
  const t = thread();
  apply(t, 2, 12);
  t.setRunning(false, events.find(e => e.seq === 17).time - 100);
  t.setRunning(true, events.find(e => e.seq === 17).time - 50);
  assert.equal(t.projection().runs[0].status, 'running');
  apply(t, 17, 17);
  assert.equal(Date.parse(t.projection().runs[0].completedAt), events.find(e => e.seq === 17).time);
});

test('legacy history uses the user message time when no send metadata was saved', () => {
  const start = 1_700_000_000_000;
  const user = { id: 'old-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] };
  for (const aborted of [false, true]) {
    const recorded = [
      { seq: 1, type: 'turn/start', time: start, data: { turn: 1 } },
      { seq: 2, type: 'user/message', time: start + 6000, data: user },
      { seq: 3, type: 'assistant/message', time: start + 39000, data: { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'text', text: 'hi' }] } } },
      { seq: 4, type: 'turn/end', time: start + 44000, data: { turn: 1, reason: aborted ? { kind: 'aborted', reason: { kind: 'user' } } : { kind: 'completed' } } },
    ];
    const live = thread(); for (const event of recorded) live.applyEvent(event);
    const before = live.projection().runs[0];
    assert.equal(seconds(before), 38, 'start at the user event, even when the engine started earlier');
    live.applyEvent({ seq: 5, type: 'turn/start', time: start + 60000, data: { turn: 2 } });
    assert.equal(seconds(live.projection().runs[0]), 38, 'a later turn cannot change the elapsed time');
    const restarted = thread(); for (const event of recorded) restarted.applyEvent(event);
    assert.deepEqual(restarted.projection().runs[0], before);
  }
});

test('C6 journal without send metadata contains only a 3.505s first turn', () => {
  const initial = thread(); apply(initial, 2, 17);
  const initialRun = initial.projection().runs[0];
  assert.equal(seconds(initialRun), 3.505);
  const user = events.find(e => e.seq === 5), start = events.find(e => e.seq === 2);
  assert.equal(user.time, start.time, 'no earlier user send event survives in this fixture');
  const restored = thread(); apply(restored, 2, 205);
  assert.equal(seconds(restored.projection().runs[0]), 3.505);
  assert.deepEqual(restored.projection().runs.slice(1, 5).map(run => Math.round(seconds(run))), [23, 28, 82, 71]);
});

test('legacy inbox delivery keeps its earliest user event when the engine starts later', () => {
  const base = 1_700_010_000_000;
  const user = { id: 'inbox-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] };
  const events = [
    { seq: 1, type: 'agent/inbox/spliced', time: base, data: { inserted: [user] } },
    { seq: 2, type: 'turn/start', time: base + 7000, data: { turn: 1 } },
    { seq: 3, type: 'user/message', time: base + 7000, data: user },
    { seq: 4, type: 'turn/end', time: base + 38000, data: { turn: 1, reason: { kind: 'aborted', reason: { kind: 'user' } } } },
  ];
  const live = thread(); for (const event of events) live.applyEvent(event);
  assert.equal(live.projection().runs.length, 1);
  assert.equal(seconds(live.projection().runs[0]), 38);
  const restarted = thread(); for (const event of events) restarted.applyEvent(event);
  assert.deepEqual(restarted.projection().runs, live.projection().runs);
});

test('legacy unfinished history freezes at its last durable event, not restart time', () => {
  const base = 1_700_020_000_000;
  const journal = [
    { seq: 1, type: 'user/message', time: base, data: { id: 'unfinished', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] } },
    { seq: 2, type: 'turn/start', time: base + 1000, data: { turn: 1 } },
    { seq: 3, type: 'assistant/message', time: base + 7000, data: { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'text', text: 'hi' }] } } },
    { seq: 4, type: 'plugin:hoh/context-usage', time: base + 9000, data: {} },
  ];
  const snapshot = at => {
    const model = thread(); journal.forEach(event => model.applyEvent(event));
    model.settleUnfinished(at);
    return model.projection().runs[0];
  };
  const first = snapshot(base + 100000);
  assert.equal(seconds(first), 9);
  assert.deepEqual(snapshot(base + 200000), first);
});

test('an unfinished legacy turn with no reply ends at its last turn/start event', () => {
  const base = 1_700_030_000_000;
  const model = thread();
  model.applyEvent({ seq: 1, type: 'user/message', time: base, data: { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hello' }] } });
  model.applyEvent({ seq: 2, type: 'turn/start', time: base + 5000, data: { turn: 1 } });
  model.settleUnfinished(base + 100000);
  assert.equal(seconds(model.projection().runs[0]), 5);
});

test('a new prompt does not extend a preceding unfinished legacy turn', () => {
  const base = 1_700_040_000_000;
  const user = id => ({ id, source: { kind: 'user' }, content: [{ type: 'text', text: id }] });
  const model = thread();
  model.applyEvent({ seq: 1, type: 'user/message', time: base, data: user('one') });
  model.applyEvent({ seq: 2, type: 'turn/start', time: base + 1000, data: { turn: 1 } });
  model.applyEvent({ seq: 3, type: 'assistant/message', time: base + 3000, data: { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'text', text: 'hi' }] } } });
  model.applyEvent({ seq: 4, type: 'turn/start', time: base + 20000, data: { turn: 2 } });
  model.applyEvent({ seq: 5, type: 'user/message', time: base + 21000, data: user('two') });
  assert.equal(seconds(model.projection().runs[0]), 3);
});

test('legacy idle without turn/end has the same duration live and after restart', () => {
  const base = 1_700_050_000_000;
  const journal = [
    { seq: 1, type: 'user/message', time: base, data: { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hi' }] } },
    { seq: 2, type: 'turn/start', time: base + 1000, data: { turn: 1 } },
    { seq: 3, type: 'assistant/message', time: base + 9000, data: { turn: 1, step: 1, message: { id: 'a', content: [{ type: 'text', text: 'hi' }] } } },
  ];
  const live = thread(); journal.forEach(event => live.applyEvent(event)); live.setRunning(false, base + 100000);
  const restored = thread(); journal.forEach(event => restored.applyEvent(event)); restored.settleUnfinished(base + 200000);
  assert.equal(seconds(live.projection().runs[0]), 9);
  assert.equal(seconds(restored.projection().runs[0]), 9);
});

test('a repeated durable user message is the last event of an unfinished legacy turn', () => {
  const base = 1_700_060_000_000;
  const user = { id: 'u', source: { kind: 'user' }, content: [{ type: 'text', text: 'hi' }] };
  const model = thread();
  model.applyEvent({ seq: 1, type: 'agent/inbox/spliced', time: base, data: { inserted: [user] } });
  model.applyEvent({ seq: 2, type: 'user/message', time: base + 9000, data: user });
  model.settleUnfinished(base + 100000);
  assert.equal(model.projection().runs.length, 1);
  assert.equal(seconds(model.projection().runs[0]), 9);
});
