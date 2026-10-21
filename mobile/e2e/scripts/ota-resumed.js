var res = http.get(CONTROL + '/control/ota/resumed');
if (res.status !== 200) throw 'the device did not pick the image up where the link dropped: ' + res.body;
