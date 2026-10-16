$summon zombie $(x) $(y) $(z) {Tags:["arnis.ob.ent","arnis.ob.mob","arnis.ob.turning"],PersistenceRequired:1b,CanBreakDoors:1b,drop_chances:{head:1.0f}}
# The loot context's `this` is the player, so the head carries their skin and name.
loot replace entity @e[tag=arnis.ob.turning,limit=1] armor.head loot arnis:outbreak/infected_head
execute as @e[tag=arnis.ob.turning] run function arnis:outbreak/mob/turned
tellraw @a ["",{"selector":"@s","color":"red"},{"text":" turned into a zombie","color":"gray"}]
