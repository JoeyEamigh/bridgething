if (PHASE === 'reset') {
  var cleared = http.get(CONTROL + '/control/store/settings-hits/reset');
  if (cleared.status !== 200) throw 'could not reset the hosted settings hit count: ' + cleared.status;
} else {
  var res = http.get(CONTROL + '/store/settings-hits');
  if (res.status !== 200) throw 'could not read the hosted settings hit count: ' + res.status;
  var hits = JSON.parse(res.body).hits;

  if (PHASE === 'before') {
    if (hits !== 0) throw 'the hosted page was fetched before the settings screen opened: ' + hits;
  } else if (hits < 1) {
    throw 'the settings page came off the device; the catalog copy was never fetched';
  }
}
