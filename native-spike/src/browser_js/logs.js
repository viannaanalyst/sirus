const input = JSON.parse(args);
const clear = input.clear;
const logs = window.__sirusLogs || [];
const result = JSON.stringify(logs.slice(-200));
if (clear) window.__sirusLogs = [];
return result;
