var res = http.get(CONTROL + '/control/bounce/mode?mode=' + MODE);
if (res.status !== 200)
  throw 'could not point the bounce root at ' + MODE + ': ' + res.status;
