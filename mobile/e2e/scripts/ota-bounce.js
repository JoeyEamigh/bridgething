var url =
  CONTROL + '/control/daemon/bounce?downMs=' + DOWN_MS + '&mode=' + MODE;
var res = http.get(url);
if (res.status !== 200) throw 'daemon bounce failed: ' + res.status;
