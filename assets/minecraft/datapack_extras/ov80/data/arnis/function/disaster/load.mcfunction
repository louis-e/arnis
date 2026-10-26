scoreboard objectives add arnis.ds dummy
scoreboard objectives add arnis.ds.seen dummy
scoreboard objectives add arnis.ds.ctl trigger
scoreboard objectives add arnis.ds.deaths deathCount
scoreboard objectives add arnis.ds.won dummy {"text":"Survived","color":"gold"}
scoreboard objectives add arnis.ds.run dummy
scoreboard objectives add arnis.ds.t dummy
scoreboard objectives add arnis.ds.id dummy
scoreboard objectives add arnis.ds.d dummy
scoreboard objectives add arnis.ds.m dummy
scoreboard objectives add arnis.ds.flood dummy
scoreboard objectives add arnis.ds.tsunami dummy
scoreboard objectives add arnis.ds.snow dummy
scoreboard players set #2 arnis 2
scoreboard players set #3 arnis 3
scoreboard players set #4 arnis 4
scoreboard players set #10 arnis 10
scoreboard players set #16 arnis 16
scoreboard players set #40 arnis 40
scoreboard players set #64 arnis 64
bossbar add arnis:disaster {"text":"Disasters"}
function arnis:disaster/area
# Scores persist with the world: only a fresh world starts out waiting for its first player.
execute unless score #state arnis.ds matches -1.. run scoreboard players set #state arnis.ds 0
execute unless score #lv_flood arnis.ds matches 0.. run scoreboard players set #lv_flood arnis.ds 0
execute unless score #lv_tsunami arnis.ds matches 0.. run scoreboard players set #lv_tsunami arnis.ds 0
execute unless score #lv_snow arnis.ds matches 0.. run scoreboard players set #lv_snow arnis.ds 0
function arnis:disaster/flood/tops
function arnis:disaster/tsunami/tops
scoreboard players set #tk arnis.ds 0
