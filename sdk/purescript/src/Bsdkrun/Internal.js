export const async = work => fail => done => () => {
  const controller = new AbortController();
  let active = true;
  Promise.resolve().then(() => active ? work(controller.signal) : undefined).then(
    value => { if (active) { active = false; done(value)(); } },
    reason => { if (active) { active = false; fail(reason instanceof Error ? reason : new Error(String(reason)))(); } },
  );
  return () => { active = false; controller.abort(); };
};
