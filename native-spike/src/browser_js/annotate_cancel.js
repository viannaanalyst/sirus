(() => {
  const current = window.__sirusAnnotations;
  if (current) current.cancel();
  return "cancelled";
})()
