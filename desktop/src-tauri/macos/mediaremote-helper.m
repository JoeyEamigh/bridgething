#import <Foundation/Foundation.h>

#include <crt_externs.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

@interface MROrigin : NSObject
+ (instancetype)localOrigin;
@end

@interface MRPlayerPath : NSObject
- (instancetype)initWithOrigin:(id)origin client:(id)client player:(id)player;
@end

@interface MRPlaybackQueueRequest : NSObject
@property(nonatomic) BOOL includeMetadata;
@property(nonatomic) NSInteger location;
@property(nonatomic) NSInteger length;
@property(nonatomic) double artworkWidth;
@property(nonatomic) double artworkHeight;
@end

@interface MRPlaybackQueue : NSObject
- (NSDictionary *)dictionaryRepresentation;
- (NSArray *)contentItems;
@end

@interface MRContentItem : NSObject
- (id)artwork;
@end

@interface MRArtwork : NSObject
- (NSData *)imageData;
@end

@interface MRClient : NSObject
- (NSString *)bundleIdentifier;
- (NSString *)parentApplicationBundleIdentifier;
@end

typedef void (*MRWantsNotifications)(Boolean);
typedef void (*MRRegisterNotifications)(dispatch_queue_t);
typedef void (*MRGetObject)(dispatch_queue_t, void (^)(id));
typedef void (*MRGetQueue)(id, id, dispatch_queue_t, void (^)(id));
typedef Boolean (*MRSendCommand)(int, CFDictionaryRef);
typedef int (*MRCommandOf)(id);
typedef Boolean (*MREnabledOf)(id);

static struct {
  MRWantsNotifications wantsNotifications;
  MRRegisterNotifications registerNotifications;
  MRGetObject client;
  MRGetObject player;
  MRGetQueue queue;
  MRGetObject supported;
  MRCommandOf commandOf;
  MREnabledOf enabledOf;
  MRSendCommand send;
} gMediaRemote;

typedef enum {
  StageSymbols = 0,
  StageClient,
  StagePlayer,
  StageQueue,
  StageArtwork,
  StageNotifications,
  StageCommands,
  StageCount,
} Stage;

static const char *const kStageNames[StageCount] = {
  "symbols", "client", "player", "queue", "artwork", "notifications", "commands",
};

static const char *kFrameworkPath = "/System/Library/PrivateFrameworks/MediaRemote.framework/MediaRemote";
static const int64_t kQueueLength = 25;
static const uint64_t kCoalesceNanos = 150 * NSEC_PER_MSEC;
static const uint64_t kQueueAnswerNanos = 1500 * NSEC_PER_MSEC;
static const uint64_t kCommandsAnswerNanos = 250 * NSEC_PER_MSEC;
static const uint64_t kPollNanos = 2 * NSEC_PER_SEC;
static const uint64_t kHeartbeatNanos = 1 * NSEC_PER_SEC;

static dispatch_queue_t gWork;
static pthread_mutex_t gOut = PTHREAD_MUTEX_INITIALIZER;
static BOOL gCoalescing;
static unsigned gSkipped;

#ifdef BRIDGETHING_MEDIAREMOTE_FAULTS
static BOOL faulted(const char *what) {
  return strstr(BRIDGETHING_MEDIAREMOTE_FAULTS, what) != NULL;
}

static void die(Stage stage) {
  char token[64];
  snprintf(token, sizeof token, "crash:%s", kStageNames[stage]);
  if (faulted(token)) {
    _exit(134);
  }
}
#else
static BOOL faulted(const char *what) {
  (void)what;
  return NO;
}

static void die(Stage stage) {
  (void)stage;
}
#endif

static void emit(NSDictionary *payload) {
  NSError *error = nil;
  NSData *encoded = [NSJSONSerialization dataWithJSONObject:payload options:0 error:&error];
  if (!encoded) {
    return;
  }
  pthread_mutex_lock(&gOut);
  fwrite(encoded.bytes, 1, encoded.length, stdout);
  fputc('\n', stdout);
  fflush(stdout);
  pthread_mutex_unlock(&gOut);
}

static BOOL skipped(Stage stage) {
  return (gSkipped & (1u << stage)) != 0;
}

static void enter(Stage stage) {
  emit(@{@"event" : @"stage", @"name" : @(kStageNames[stage])});
  die(stage);
}

static void *symbol(void *framework, const char *name) {
  return faulted(name) ? NULL : dlsym(framework, name);
}

static NSDictionary *dictionary(id value) {
  return [value isKindOfClass:NSDictionary.class] ? (NSDictionary *)value : nil;
}

static NSNumber *number(id value) {
  return [value isKindOfClass:NSNumber.class] ? (NSNumber *)value : nil;
}

static NSString *filled(id value) {
  return [value isKindOfClass:NSString.class] && [(NSString *)value length] > 0 ? (NSString *)value : nil;
}

static NSString *text(NSDictionary *from, NSString *key) {
  return filled(from[key]);
}

static NSNumber *quantity(id value) {
  NSNumber *plain = number(value);
  if (plain) {
    return plain;
  }
  if (![value isKindOfClass:NSString.class]) {
    return nil;
  }
  NSString *body = (NSString *)value;
  NSRange open = [body rangeOfString:@"("];
  if (open.location == NSNotFound) {
    if ([body containsString:@":"]) {
      return nil;
    }
  } else {
    body = [body substringFromIndex:open.location + 1];
  }
  double parsed = 0;
  return [[NSScanner scannerWithString:body] scanDouble:&parsed] ? @(parsed) : nil;
}

static NSNumber *milliseconds(id value) {
  NSNumber *count = quantity(value);
  return count ? @((int64_t)(count.doubleValue * 1000.0)) : nil;
}

static NSNumber *unixMilliseconds(id value) {
  if ([value isKindOfClass:NSDate.class]) {
    return @((int64_t)([(NSDate *)value timeIntervalSince1970] * 1000.0));
  }
  NSNumber *stamp = number(value);
  return stamp ? @((int64_t)((stamp.doubleValue + kCFAbsoluteTimeIntervalSince1970) * 1000.0)) : nil;
}

static void put(NSMutableDictionary *into, NSString *key, id value) {
  into[key] = value ?: NSNull.null;
}

static NSDictionary *representationOf(id queue) {
  if (![queue respondsToSelector:@selector(dictionaryRepresentation)]) {
    return nil;
  }
  @try {
    if (faulted("dictionaryRepresentation")) {
      [NSException raise:NSInvalidArgumentException format:@"dictionaryRepresentation"];
    }
    return dictionary([(MRPlaybackQueue *)queue dictionaryRepresentation]);
  } @catch (NSException *refused) {
    return nil;
  }
}

static NSData *artworkOf(id queue) {
  if (![queue respondsToSelector:@selector(contentItems)]) {
    return nil;
  }
  @try {
    id item = [(MRPlaybackQueue *)queue contentItems].firstObject;
    if (![item respondsToSelector:@selector(artwork)]) {
      return nil;
    }
    id artwork = [(MRContentItem *)item artwork];
    if (![artwork respondsToSelector:@selector(imageData)]) {
      return nil;
    }
    NSData *bytes = [(MRArtwork *)artwork imageData];
    return [bytes isKindOfClass:NSData.class] && bytes.length > 0 ? bytes : nil;
  } @catch (NSException *refused) {
    return nil;
  }
}

static NSString *packageOf(id client) {
  MRClient *app = client;
  @try {
    NSString *own = [app respondsToSelector:@selector(bundleIdentifier)] ? filled(app.bundleIdentifier) : nil;
    NSString *parent = [app respondsToSelector:@selector(parentApplicationBundleIdentifier)]
                         ? filled(app.parentApplicationBundleIdentifier)
                         : nil;
    return own && [own hasPrefix:@"com.apple.WebKit."] && parent ? parent : (own ?: parent);
  } @catch (NSException *refused) {
    return nil;
  }
}

static MRPlaybackQueueRequest *request(int64_t location, int64_t length, double artworkEdge) {
  MRPlaybackQueueRequest *ask = [[NSClassFromString(@"MRPlaybackQueueRequest") alloc] init];
  if (![ask respondsToSelector:@selector(setIncludeMetadata:)] || ![ask respondsToSelector:@selector(setLocation:)] ||
      ![ask respondsToSelector:@selector(setLength:)]) {
    return nil;
  }
  @try {
    ask.includeMetadata = YES;
    ask.location = (NSInteger)location;
    ask.length = (NSInteger)length;
    if (artworkEdge > 0 && !skipped(StageArtwork) && [ask respondsToSelector:@selector(setArtworkWidth:)] &&
        [ask respondsToSelector:@selector(setArtworkHeight:)]) {
      enter(StageArtwork);
      ask.artworkWidth = artworkEdge;
      ask.artworkHeight = artworkEdge;
    }
  } @catch (NSException *refused) {
    return nil;
  }
  return ask;
}

static id playerPath(id client, id player) {
  Class origin = NSClassFromString(@"MROrigin");
  Class path = NSClassFromString(@"MRPlayerPath");
  if (![origin respondsToSelector:@selector(localOrigin)] ||
      ![path instancesRespondToSelector:@selector(initWithOrigin:client:player:)]) {
    return nil;
  }
  @try {
    return [[path alloc] initWithOrigin:[origin localOrigin] client:client player:player];
  } @catch (NSException *refused) {
    return nil;
  }
}

static void withQueue(int64_t location, int64_t length, double artworkEdge, void (^answer)(id client, id queue)) {
  __block BOOL settled = NO;
  __block id caught = nil;
  void (^once)(id, id) = ^(id client, id queue) {
    if (!settled) {
      settled = YES;
      answer(client, queue);
    }
  };
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)kQueueAnswerNanos), gWork, ^{
    once(caught, nil);
  });
  if (!gMediaRemote.client || skipped(StageClient)) {
    once(nil, nil);
    return;
  }
  enter(StageClient);
  gMediaRemote.client(gWork, ^(id client) {
    caught = client;
    if (!client || !gMediaRemote.queue || skipped(StageQueue)) {
      once(client, nil);
      return;
    }
    void (^ask)(id) = ^(id player) {
      id path = playerPath(client, player);
      MRPlaybackQueueRequest *carried = request(location, length, artworkEdge);
      if (!path || !carried) {
        once(client, nil);
        return;
      }
      enter(StageQueue);
      gMediaRemote.queue(carried, path, gWork, ^(id queue) {
        once(client, queue);
      });
    };
    if (!gMediaRemote.player || skipped(StagePlayer)) {
      ask(nil);
      return;
    }
    enter(StagePlayer);
    gMediaRemote.player(gWork, ask);
  });
}

static void withCommands(void (^answer)(NSArray *commands)) {
  __block BOOL settled = NO;
  void (^once)(NSArray *) = ^(NSArray *commands) {
    if (!settled) {
      settled = YES;
      answer(commands);
    }
  };
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)kCommandsAnswerNanos), gWork, ^{
    once(nil);
  });
  if (!gMediaRemote.supported || !gMediaRemote.commandOf || !gMediaRemote.enabledOf || skipped(StageCommands)) {
    once(nil);
    return;
  }
  enter(StageCommands);
  gMediaRemote.supported(gWork, ^(id supported) {
    if (![supported isKindOfClass:NSArray.class]) {
      once(nil);
      return;
    }
    NSMutableArray *claimed = [NSMutableArray arrayWithCapacity:[supported count]];
    @try {
      for (id info in supported) {
        if (gMediaRemote.enabledOf(info)) {
          [claimed addObject:@(gMediaRemote.commandOf(info))];
        }
      }
    } @catch (NSException *refused) {
      once(nil);
      return;
    }
    once(claimed);
  });
}

static NSArray *itemsOf(NSDictionary *representation) {
  id items = representation[@"contentItems"];
  return [items isKindOfClass:NSArray.class] ? (NSArray *)items : nil;
}

static NSInteger activeIndex(NSDictionary *representation) {
  NSArray *items = itemsOf(representation);
  NSNumber *location = number(representation[@"location"]);
  NSInteger at = location ? location.integerValue : -1;
  if (at >= 0 && at < (NSInteger)items.count) {
    return at;
  }
  return items.count > 0 ? 0 : -1;
}

static NSDictionary *activeItem(NSDictionary *representation) {
  NSInteger at = activeIndex(representation);
  return at < 0 ? nil : dictionary(itemsOf(representation)[at]);
}

static NSString *identity(id value) {
  NSNumber *counted = number(value);
  return counted ? counted.stringValue : filled(value);
}

static NSArray *entriesOf(NSDictionary *representation) {
  NSMutableArray *entries = [NSMutableArray array];
  for (id item in itemsOf(representation)) {
    NSDictionary *held = dictionary(item);
    NSDictionary *metadata = dictionary(held[@"metadata"]);
    NSMutableDictionary *entry = [NSMutableDictionary dictionary];
    put(entry, @"id", identity(held[@"identifier"]));
    put(entry, @"title", text(metadata, @"title"));
    put(entry, @"subtitle", text(metadata, @"trackArtistName"));
    put(entry, @"artworkId", text(metadata, @"artworkIdentifier"));
    [entries addObject:entry];
  }
  return entries;
}

static NSMutableDictionary *stateOf(id client, id queue) {
  NSString *package = packageOf(client);
  NSDictionary *representation = representationOf(queue);
  NSDictionary *metadata = dictionary(activeItem(representation)[@"metadata"]);
  NSString *title = text(metadata, @"title");
  NSString *artist = text(metadata, @"trackArtistName");
  NSString *album = text(metadata, @"albumName");
  if (!package || (!title && !artist && !album)) {
    return nil;
  }
  NSNumber *rate = quantity(metadata[@"playbackRate"]);
  NSInteger at = activeIndex(representation);
  NSMutableDictionary *state = [NSMutableDictionary dictionary];
  state[@"event"] = @"state";
  state[@"package"] = package;
  state[@"playing"] = @((BOOL)(rate.doubleValue > 0));
  state[@"queue"] = entriesOf(representation);
  put(state, @"title", title);
  put(state, @"artist", artist);
  put(state, @"album", album);
  put(state, @"durationMs", milliseconds(metadata[@"duration"]));
  put(state, @"elapsedMs", milliseconds(metadata[@"elapsedTime"]));
  put(state, @"timestampUnixMs", unixMilliseconds(metadata[@"elapsedTimeTimestamp"]));
  put(state, @"rate", rate);
  put(state, @"artworkId", text(metadata, @"artworkIdentifier"));
  put(state, @"activeIndex", at < 0 ? nil : @(at));
  put(state, @"queueTitle", text(dictionary(metadata[@"collectionInfo"]),
                                 @"kMRMediaRemoteNowPlayingCollectionInfoKeyTitle"));
  return state;
}

static void publish(void) {
  withQueue(0, kQueueLength, 0, ^(id client, id queue) {
    NSMutableDictionary *state = stateOf(client, queue);
    if (!state) {
      emit(@{@"event" : @"none"});
      return;
    }
    withCommands(^(NSArray *commands) {
      put(state, @"commands", commands);
      emit(state);
    });
  });
}

static void schedulePublish(void) {
  dispatch_async(gWork, ^{
    if (gCoalescing) {
      return;
    }
    gCoalescing = YES;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)kCoalesceNanos), gWork, ^{
      gCoalescing = NO;
      publish();
    });
  });
}

static NSInteger artworkIndex(NSDictionary *representation, NSString *token) {
  NSArray *items = itemsOf(representation);
  for (NSInteger at = 0; at < (NSInteger)items.count; at++) {
    NSString *identifier = text(dictionary(dictionary(items[at])[@"metadata"]), @"artworkIdentifier");
    if (identifier && [identifier isEqualToString:token]) {
      return at;
    }
  }
  return -1;
}

static void emitArt(NSString *token, id queue) {
  NSDictionary *metadata = dictionary(activeItem(representationOf(queue))[@"metadata"]);
  NSData *bytes = artworkOf(queue);
  NSMutableDictionary *reply = [NSMutableDictionary dictionary];
  reply[@"event"] = @"art";
  put(reply, @"token", token);
  put(reply, @"artworkId", text(metadata, @"artworkIdentifier"));
  put(reply, @"mime", text(metadata, @"artworkMIMEType"));
  put(reply, @"base64", bytes ? [bytes base64EncodedStringWithOptions:0] : nil);
  emit(reply);
}

static void answerArt(NSString *token, double edge) {
  if (skipped(StageArtwork)) {
    emitArt(token, nil);
    return;
  }
  withQueue(0, kQueueLength, 0, ^(id client, id queue) {
    NSInteger at = artworkIndex(representationOf(queue), token);
    if (at < 0) {
      emitArt(token, nil);
      return;
    }
    withQueue(at, 1, edge, ^(id owner, id page) {
      emitArt(token, page);
    });
  });
}

static void sendCommand(int identifier, NSDictionary *options) {
  if (!gMediaRemote.send) {
    return;
  }
  @try {
    NSMutableDictionary *carried = [NSMutableDictionary dictionary];
    for (NSString *key in options) {
      id value = options[key];
      BOOL carriable = [value isKindOfClass:NSNumber.class] || [value isKindOfClass:NSString.class];
      if ([key isKindOfClass:NSString.class] && carriable) {
        carried[key] = value;
      }
    }
    gMediaRemote.send(identifier, (__bridge CFDictionaryRef)carried);
  } @catch (NSException *refused) {
  }
}

static void accept(NSDictionary *line) {
  NSString *kind = line[@"cmd"];
  if ([kind isEqualToString:@"send"]) {
    sendCommand(((NSNumber *)line[@"id"]).intValue,
                [line[@"options"] isKindOfClass:NSDictionary.class] ? line[@"options"] : @{});
    schedulePublish();
  } else if ([kind isEqualToString:@"art"]) {
    NSNumber *edge = number(line[@"edge"]);
    answerArt([line[@"token"] isKindOfClass:NSString.class] ? line[@"token"] : nil, edge ? edge.doubleValue : 512.0);
  } else if ([kind isEqualToString:@"state"]) {
    schedulePublish();
  }
}

static void *readCommands(void *unused) {
  char *line = NULL;
  size_t capacity = 0;
  ssize_t read = 0;
  while ((read = getline(&line, &capacity, stdin)) > 0) {
    NSData *raw = [NSData dataWithBytes:line length:(NSUInteger)read];
    NSDictionary *parsed = [NSJSONSerialization JSONObjectWithData:raw options:0 error:NULL];
    if ([parsed isKindOfClass:NSDictionary.class]) {
      dispatch_async(gWork, ^{
        accept(parsed);
      });
    }
  }
  free(line);
  _exit(0);
}

static void observe(void) {
  NSArray *names = @[
    @"kMRMediaRemoteNowPlayingInfoDidChangeNotification",
    @"kMRMediaRemoteNowPlayingApplicationIsPlayingDidChangeNotification",
    @"kMRMediaRemoteNowPlayingApplicationPlaybackStateDidChangeNotification",
    @"kMRMediaRemoteNowPlayingApplicationDidChangeNotification",
    @"kMRMediaRemoteNowPlayingPlaybackQueueDidChangeNotification",
    @"kMRNowPlayingPlaybackQueueChangedNotification",
    @"kMRPlaybackQueueContentItemsChangedNotification",
    @"kMRPlayerPlaybackQueueChangedNotification",
    @"kMRPlaybackQueueContentItemArtworkChangedNotification",
    @"kMRMediaRemoteSupportedCommandsDidChangeNotification",
    @"kMRMediaRemotePlayerSupportedCommandsDidChangeNotification",
  ];
  for (NSString *name in names) {
    [NSNotificationCenter.defaultCenter addObserverForName:name
                                                    object:nil
                                                     queue:nil
                                                usingBlock:^(NSNotification *note) {
                                                  schedulePublish();
                                                }];
  }
}

static void heartbeat(void) {
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)kHeartbeatNanos), gWork, ^{
    emit(@{@"event" : @"tick"});
    heartbeat();
  });
}

static void poll(void) {
  dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)kPollNanos), gWork, ^{
    schedulePublish();
    poll();
  });
}

static void readSkipList(void) {
  int argc = *_NSGetArgc();
  const char *const *argv = (const char *const *)*_NSGetArgv();
  for (int at = 0; at < argc; at++) {
    if (!argv[at] || strncmp(argv[at], "skip=", 5) != 0) {
      continue;
    }
    for (int stage = 0; stage < StageCount; stage++) {
      if (strstr(argv[at] + 5, kStageNames[stage])) {
        gSkipped |= 1u << stage;
      }
    }
  }
}

static BOOL resolve(void) {
  if (skipped(StageSymbols)) {
    return NO;
  }
  enter(StageSymbols);
  void *framework = dlopen(kFrameworkPath, RTLD_NOW);
  if (!framework) {
    return NO;
  }
  gMediaRemote.wantsNotifications =
    (MRWantsNotifications)symbol(framework, "MRMediaRemoteSetWantsNowPlayingNotifications");
  gMediaRemote.registerNotifications =
    (MRRegisterNotifications)symbol(framework, "MRMediaRemoteRegisterForNowPlayingNotifications");
  gMediaRemote.client = (MRGetObject)symbol(framework, "MRMediaRemoteGetNowPlayingClient");
  gMediaRemote.player = (MRGetObject)symbol(framework, "MRMediaRemoteGetNowPlayingPlayer");
  gMediaRemote.queue = (MRGetQueue)symbol(framework, "MRMediaRemoteRequestNowPlayingPlaybackQueueForPlayer");
  gMediaRemote.supported = (MRGetObject)symbol(framework, "MRMediaRemoteGetSupportedCommands");
  gMediaRemote.commandOf = (MRCommandOf)symbol(framework, "MRMediaRemoteCommandInfoGetCommand");
  gMediaRemote.enabledOf = (MREnabledOf)symbol(framework, "MRMediaRemoteCommandInfoGetEnabled");
  gMediaRemote.send = (MRSendCommand)symbol(framework, "MRMediaRemoteSendCommand");
  return YES;
}

__attribute__((constructor)) static void bridgethingMediaRemoteStart(void) {
  gWork = dispatch_queue_create("com.bridgething.mediaremote", DISPATCH_QUEUE_SERIAL);
  readSkipList();

  if (resolve()) {
    if (skipped(StageNotifications)) {
      poll();
    } else {
      enter(StageNotifications);
      if (gMediaRemote.wantsNotifications) {
        gMediaRemote.wantsNotifications(true);
      }
      if (gMediaRemote.registerNotifications) {
        gMediaRemote.registerNotifications(gWork);
      }
      observe();
    }
  }

  pthread_t reader;
  if (pthread_create(&reader, NULL, readCommands, NULL) != 0) {
    _exit(1);
  }
  pthread_detach(reader);

  heartbeat();
  schedulePublish();
}
