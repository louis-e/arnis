# The marker riding it is how its death is noticed: it is dropped off then, but unloads along with a living boss.
summon zombie ~ ~ ~ {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.boss","arnis.ob.new"],PersistenceRequired:1b,CanBreakDoors:1b,Glowing:1b,CustomName:{"text":"Patient Zero","color":"dark_red"},Passengers:[{id:"minecraft:marker",Tags:["arnis.ob.ent","arnis.ob.heart"]}]}
execute as @e[tag=arnis.ob.boss,tag=arnis.ob.new] run function arnis:outbreak/boss/setup
particle minecraft:large_smoke ~ ~1 ~ 1 2 1 0.05 200 force
title @a times 10 70 20
title @a subtitle {"text":"Kill it to end the outbreak","color":"gray"}
title @a title {"text":"Patient Zero","color":"dark_red"}
execute as @a at @s run playsound minecraft:entity.wither.spawn master @s ~ ~ ~ 0.8 0.6
