var res = http.get(CONTROL + '/control/daemon/reboot?image=' + IMAGE);
if (res.status !== 200) throw 'could not reboot the device onto ' + IMAGE + ': ' + res.status;
