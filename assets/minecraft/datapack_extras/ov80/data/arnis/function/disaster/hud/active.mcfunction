bossbar set arnis:disaster color red
execute if score #len arnis.ds matches 1.. run function arnis:disaster/hud/timed
execute if score #len arnis.ds matches ..0 run function arnis:disaster/hud/status
