scoreboard players set #ob_t arnis.ob 0
execute if score #state arnis.ob matches 0 if entity @a run function arnis:outbreak/start
execute if score #state arnis.ob matches 1 run function arnis:outbreak/running
# The infected of a stopped or restarted outbreak are cleared as their chunks load.
execute unless score #state arnis.ob matches 1 run kill @e[tag=arnis.ob.ent]
execute if score #state arnis.ob matches 1 run function arnis:outbreak/sweep
