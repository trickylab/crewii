import { dirname } from 'node:path';
import { emptyState } from './core.js';

const pending = new Map();

export function createStore(file, { fs, now, onReset = () => {} }) {
  let recovered;
  function read() {
    let content;
    try {
      content = fs.readFileSync(file, 'utf8');
    } catch (error) {
      if (error.code === 'ENOENT') return recovered ?? emptyState();
      throw error;
    }
    try {
      const state = JSON.parse(content);
      if (state?.version > 1) throw Error(`state file version ${state.version} is newer than this crewii; upgrade crewii`);
      if (state && state.version === 1 && Number.isSafeInteger(state.seq) && state.seq >= 0 && Number.isSafeInteger(state.nextTicket) && state.nextTicket >= 1 && state.instances && typeof state.instances === 'object' && !Array.isArray(state.instances) && state.tickets && typeof state.tickets === 'object' && !Array.isArray(state.tickets) && Array.isArray(state.events)) return state;
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error;
    }
    fs.renameSync(file, `${file}.corrupt-${now()}`);
    const max = [...content.matchAll(/T-(\d+)/g)].reduce((n, match) => Math.max(n, Number(match[1]) < Number.MAX_SAFE_INTEGER ? Number(match[1]) : 0), 0);
    onReset();
    return recovered = { ...emptyState(), nextTicket: max + 1 };
  }

  function update(fn) {
    const result = (pending.get(file) ?? Promise.resolve()).then(async () => {
      const state = await fn(read());
      fs.mkdirSync(dirname(file), { recursive: true });
      fs.writeFileSync(`${file}.tmp`, JSON.stringify(state), { mode: 0o600 });
      fs.chmodSync(`${file}.tmp`, 0o600);
      fs.renameSync(`${file}.tmp`, file);
      recovered = undefined;
      return state;
    });
    const settled = result.catch(() => {});
    pending.set(file, settled);
    settled.then(() => { if (pending.get(file) === settled) pending.delete(file); });
    return result;
  }

  return { read, update };
}
