scoreboard players remove #spawn_left arnis.ob 1
execute store result score #r arnis.ob run random value 1..100
function arnis:outbreak/wave/pick
summon marker ~ ~ ~ {Tags:["arnis.ob.spot"]}
execute as @e[type=marker,tag=arnis.ob.spot] run function arnis:outbreak/wave/spot
