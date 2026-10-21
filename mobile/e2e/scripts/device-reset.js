var res = http.get(CONTROL + '/control/device/reset');
if (res.status !== 200) throw 'could not reset the device: ' + res.status;
