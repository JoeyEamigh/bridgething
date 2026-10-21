var res = http.get(CONTROL + '/control/daemon/interrupt?downMs=' + DOWN_MS + '&mode=' + MODE);
if (res.status !== 200) throw 'could not arm the link drop: ' + res.status;
