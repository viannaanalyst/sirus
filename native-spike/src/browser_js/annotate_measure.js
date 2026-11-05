(() => {
  const current = window.__sirusAnnotations;
  const empty = { url: location.href, width: window.innerWidth, height: window.innerHeight, entries: [] };
  return JSON.stringify(current && current.measure ? current.measure() : empty);
})()
