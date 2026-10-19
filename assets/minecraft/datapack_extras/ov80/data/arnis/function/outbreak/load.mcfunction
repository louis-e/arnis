scoreboard objectives add arnis.ob dummy
scoreboard objectives add arnis.ob.kills totalKillCount {"text":"Kills","color":"red"}
scoreboard objectives add arnis.ob.deaths deathCount
scoreboard objectives add arnis.ob.ctl trigger
scoreboard objectives add arnis.ob.seen dummy
scoreboard objectives add arnis.ob.run dummy
bossbar add arnis:outbreak {"text":"Zombie Outbreak"}
bossbar add arnis:boss {"text":"Patient Zero","color":"dark_red"}
bossbar set arnis:boss color red
bossbar set arnis:boss style notched_10
bossbar set arnis:boss max 400
# Scores persist with the world: only a fresh world starts out waiting for its first player.
execute unless score #state arnis.ob matches -1.. run scoreboard players set #state arnis.ob 0
scoreboard players set #ob_t arnis.ob 0
