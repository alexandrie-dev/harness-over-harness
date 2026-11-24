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
