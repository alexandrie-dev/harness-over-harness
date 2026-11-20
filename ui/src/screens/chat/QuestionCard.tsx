import { useState, type FormEvent } from 'react';
import { useChatStore } from '../../lib/store';
import type { PendingQuestion } from './transport';

/** One question of the assistant (ask-user tool): choose options and/or write an answer. Plain text only. */
export function QuestionCard({ question }: { question: PendingQuestion }) {
  const { answerQuestion, dismissQuestion } = useChatStore.getState();
  const [selected, setSelected] = useState<Record<string, string[]>>({});
  const [custom, setCustom] = useState<Record<string, string>>({});
  const [sending, setSending] = useState(false);
  const complete = question.questions.every(q => (selected[q.id]?.length ?? 0) > 0 || (custom[q.id] ?? '').trim());
  function toggle(id: string, label: string, multi: boolean) {
    setSelected(current => {
      const chosen = current[id] ?? [];
      return { ...current, [id]: multi ? (chosen.includes(label) ? chosen.filter(l => l !== label) : [...chosen, label]) : [label] };
    });
  }
  async function submit(event: FormEvent) {
    event.preventDefault(); if (!complete || sending) return;
    setSending(true);
    await answerQuestion(question.requestId, { answers: question.questions.map(q => {
      const text = (custom[q.id] ?? '').trim();
      return { id: q.id, selected: selected[q.id] ?? [], ...(text ? { custom: text } : {}) };
    }) });
    setSending(false);
  }
  async function skip() { setSending(true); await dismissQuestion(question.requestId); setSending(false); }
  return <form className="chat-question" data-chat-question onSubmit={submit} aria-label="Вопрос Помощника">
    <p className="chat-question-head">Нужен ваш ответ</p>
    {question.questions.map(q => <fieldset className="chat-question-item" key={q.id} data-question-id={q.id}>
      <legend>{q.header ? <small>{q.header}</small> : null}{q.question}</legend>
      {q.detail && <p className="chat-question-detail">{q.detail}</p>}
      {q.options?.map(option => <label className="chat-question-option" key={option.label}>
        <input type={q.multiSelect ? 'checkbox' : 'radio'} name={`${question.requestId}:${q.id}`} checked={selected[q.id]?.includes(option.label) ?? false}
          onChange={() => toggle(q.id, option.label, q.multiSelect === true)} />
        <span>{option.label}{option.description ? <small>{option.description}</small> : null}</span>
      </label>)}
      <input className="chat-question-custom" type="text" maxLength={2000} value={custom[q.id] ?? ''} aria-label={q.options?.length ? 'Свой ответ' : 'Ответ'}
        placeholder={q.options?.length ? 'Свой вариант (необязательно)' : 'Ваш ответ'} onChange={event => setCustom(current => ({ ...current, [q.id]: event.target.value }))} />
    </fieldset>)}
    <div className="chat-question-actions">
      <button className="btn" type="button" data-question-skip disabled={sending} onClick={() => void skip()}>Пропустить</button>
      <button className="btn btn-primary" type="submit" data-question-send disabled={!complete || sending}>Ответить</button>
    </div>
  </form>;
}
