scoreboard players operation #left arnis.ob = #spawn_left arnis.ob
scoreboard players operation #left arnis.ob += #alive arnis.ob
bossbar set arnis:outbreak color red
execute store result bossbar arnis:outbreak max run scoreboard players get #wave_total arnis.ob
execute store result bossbar arnis:outbreak value run scoreboard players get #left arnis.ob
execute if score #cleared arnis.ob matches 0 run bossbar set arnis:outbreak name ["",{"text":"Night "},{"score":{"name":"#night","objective":"arnis.ob"}},{"text":" - "},{"score":{"name":"#left","objective":"arnis.ob"}},{"text":" zombies left"}]
execute if score #cleared arnis.ob matches 1 run bossbar set arnis:outbreak name ["",{"text":"Night "},{"score":{"name":"#night","objective":"arnis.ob"}},{"text":" - hold out until dawn"}]
