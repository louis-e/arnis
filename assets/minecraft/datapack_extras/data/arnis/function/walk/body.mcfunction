# Before 1.21.9 there are no mannequins: villagers walk instead.
$summon minecraft:villager $(x) $(y) $(z) {Tags:["arnis.walker","arnis.walk_new"],NoAI:1b,Silent:1b,Invulnerable:1b,PersistenceRequired:1b,NoGravity:1b,VillagerData:{type:"$(vt)",profession:"$(vp)",level:1},attributes:[{id:"minecraft:scale",base:$(size)}]}
