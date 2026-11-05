(() => {
  const current = window.__sirusAnnotations;
  if (!current) return "[]";
  const raw = JSON.stringify(current.entries || []);
  current.cancel();
  return raw;
})()
